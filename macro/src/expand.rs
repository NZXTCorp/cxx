use crate::namespace::Namespace;
use crate::syntax::atom::Atom::{self, *};
use crate::syntax::mangled::ToMangled;
use crate::syntax::{self, check, Api, ExternFn, ExternType, Struct, Type, Types};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{spanned::Spanned, Error, ItemMod, Result, Token};

pub fn bridge(namespace: &Namespace, ffi: ItemMod) -> Result<TokenStream> {
    let ident = &ffi.ident;
    let content = ffi.content.ok_or(Error::new(
        Span::call_site(),
        "#[cxx::bridge] module must have inline contents",
    ))?;
    let apis = syntax::parse_items(content.1)?;
    let ref types = Types::collect(&apis)?;
    check::typecheck(&apis, types)?;

    let mut expanded = TokenStream::new();
    let mut hidden = TokenStream::new();
    let mut has_rust_type = false;

    // "Header" to define newtypes locally so we can implement
    // traits on them.
    expanded.extend(quote! {
        pub struct Vector<T>(pub ::cxx::RealVector<T>);
        impl<T: cxx::private::VectorTarget<T>> Vector<T> {
            pub fn size(&self) -> usize {
                self.0.size()
            }
            pub fn get(&self, pos: usize) -> Option<&T> {
                self.0.get(pos)
            }
            pub fn get_unchecked(&self, pos: usize) -> &T {
                self.0.get_unchecked(pos)
            }
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
            pub fn push_back(&mut self, item: &T) {
                self.0.push_back(item)
            }
        }
        impl<'a, T: cxx::private::VectorTarget<T>> IntoIterator for &'a Vector<T> {
            type Item = &'a T;
            type IntoIter = <&'a ::cxx::RealVector<T> as IntoIterator>::IntoIter;

            fn into_iter(self) -> Self::IntoIter {
                self.0.into_iter()
            }
        }
    });

    for api in &apis {
        if let Api::RustType(ety) = api {
            expanded.extend(expand_rust_type(ety));
            if !has_rust_type {
                hidden.extend(quote!(
                    const fn __assert_sized<T>() {}
                ));
                has_rust_type = true;
            }
            let ident = &ety.ident;
            hidden.extend(quote!(__assert_sized::<#ident>();));
        }
    }

    for api in &apis {
        match api {
            Api::Include(_) | Api::RustType(_) => {}
            Api::Struct(strct) => expanded.extend(expand_struct(strct)),
            Api::CxxType(ety) => expanded.extend(expand_cxx_type(ety)),
            Api::CxxFunction(efn) => {
                expanded.extend(expand_cxx_function_shim(namespace, efn, types));
            }
            Api::RustFunction(efn) => {
                hidden.extend(expand_rust_function_shim(namespace, efn, types))
            }
        }
    }

    for ty in types {
        if let Type::RustBox(ty) = ty {
            if let Type::Ident(ident) = &ty.inner {
                if Atom::from(ident).is_none() {
                    hidden.extend(expand_rust_box(namespace, ident));
                }
            }
        } else if let Type::RustVec(ty) = ty {
            if let Type::Ident(ident) = &ty.inner {
                hidden.extend(expand_rust_vec(namespace, &ty.inner, ident));
            }
        } else if let Type::UniquePtr(ptr) = ty {
            if let Type::Ident(ident) = &ptr.inner {
                if Atom::from(ident).is_none() {
                    expanded.extend(expand_unique_ptr(namespace, &ptr.inner));
                }
            } else if let Type::Vector(_) = &ptr.inner {
                // Generate code for unique_ptr<vector<T>> if T is not an atom
                // or if T is a u8.
                // Code for atoms is already generated
                if Atom::from(ident).is_none() || Atom::from(ident) == Some(Atom::U8) {
                    expanded.extend(expand_unique_ptr(namespace, &ptr.inner));
                }
            }
        } else if let Type::Vector(ptr) = ty {
            if let Type::Ident(ident) = &ptr.inner {
                if Atom::from(ident).is_none() {
                    // Generate code for Vector<T> if T is not an atom
                    // Code for atoms is already generated
                    expanded.extend(expand_vector(namespace, &ptr.inner));
                }
            }
        }
    }

    // Work around https://github.com/rust-lang/rust/issues/67851.
    if !hidden.is_empty() {
        expanded.extend(quote! {
            #[doc(hidden)]
            const _: () = {
                #hidden
            };
        });
    }

    let attrs = ffi
        .attrs
        .into_iter()
        .filter(|attr| attr.path.is_ident("doc"));
    let vis = &ffi.vis;

    Ok(quote! {
        #(#attrs)*
        #[deny(improper_ctypes)]
        #[allow(non_snake_case)]
        #vis mod #ident {
            #expanded
        }
    })
}

fn expand_struct(strct: &Struct) -> TokenStream {
    let ident = &strct.ident;
    let doc = &strct.doc;
    let derives = &strct.derives;
    let fields = strct.fields.iter().map(|field| {
        // This span on the pub makes "private type in public interface" errors
        // appear in the right place.
        let vis = Token![pub](field.ident.span());
        quote!(#vis #field)
    });
    quote! {
        #doc
        #[derive(#(#derives),*)]
        #[repr(C)]
        pub struct #ident {
            #(#fields,)*
        }
    }
}

fn expand_cxx_type(ety: &ExternType) -> TokenStream {
    let ident = &ety.ident;
    let doc = &ety.doc;
    quote! {
        #doc
        #[repr(C)]
        pub struct #ident {
            _private: ::cxx::private::Opaque,
        }
    }
}

fn expand_cxx_function_decl(namespace: &Namespace, efn: &ExternFn, types: &Types) -> TokenStream {
    let ident = &efn.ident;
    let args = efn.args.iter().map(|arg| {
        let ident = &arg.ident;
        let ty = expand_extern_type(&arg.ty);
        if arg.ty == RustString {
            quote!(#ident: *const #ty)
        } else if types.needs_indirect_abi(&arg.ty) {
            quote!(#ident: *mut #ty)
        } else {
            quote!(#ident: #ty)
        }
    });
    let ret = expand_extern_return_type(&efn.ret, types);
    let mut outparam = None;
    if indirect_return(&efn.ret, types) {
        let ret = expand_extern_type(efn.ret.as_ref().unwrap());
        outparam = Some(quote!(__return: *mut #ret));
    }
    let link_name = format!("{}cxxbridge02${}", namespace, ident);
    let local_name = format_ident!("__{}", ident);
    quote! {
        #[link_name = #link_name]
        fn #local_name(#(#args,)* #outparam) #ret;
    }
}

fn expand_cxx_function_shim(namespace: &Namespace, efn: &ExternFn, types: &Types) -> TokenStream {
    let ident = &efn.ident;
    let doc = &efn.doc;
    let decl = expand_cxx_function_decl(namespace, efn, types);
    let args = &efn.args;
    let ret = expand_return_type(&efn.ret);
    let indirect_return = indirect_return(&efn.ret, types);
    let vars = efn.args.iter().map(|arg| {
        let var = &arg.ident;
        match &arg.ty {
            Type::Ident(ident) if ident == RustString => {
                quote!(#var.as_mut_ptr() as *const ::cxx::private::RustString)
            }
            Type::RustBox(_) => quote!(::std::boxed::Box::into_raw(#var)),
            Type::UniquePtr(_) => quote!(::cxx::UniquePtr::into_raw(#var)),
            Type::RustVec(_) => quote!(::cxx::RustVec::from(#var)),
            Type::Ref(ty) => match &ty.inner {
                Type::Ident(ident) if ident == RustString => {
                    quote!(::cxx::private::RustString::from_ref(#var))
                }
                Type::RustVec(_) => quote!(::cxx::RustVec::from_ref(#var)),
                _ => quote!(#var),
            },
            Type::Str(_) => quote!(::cxx::private::RustStr::from(#var)),
            ty if types.needs_indirect_abi(ty) => quote!(#var.as_mut_ptr()),
            _ => quote!(#var),
        }
    });
    let mut setup = efn
        .args
        .iter()
        .filter(|arg| types.needs_indirect_abi(&arg.ty))
        .map(|arg| {
            let var = &arg.ident;
            // These are arguments for which C++ has taken ownership of the data
            // behind the mut reference it received.
            quote! {
                let mut #var = std::mem::MaybeUninit::new(#var);
            }
        })
        .collect::<TokenStream>();
    let local_name = format_ident!("__{}", ident);
    let call = if indirect_return {
        let ret = expand_extern_type(efn.ret.as_ref().unwrap());
        setup.extend(quote! {
            let mut __return = ::std::mem::MaybeUninit::<#ret>::uninit();
            #local_name(#(#vars,)* __return.as_mut_ptr());
        });
        quote! {
            __return.assume_init()
        }
    } else {
        quote! {
            #local_name(#(#vars),*)
        }
    };
    let expr = efn
        .ret
        .as_ref()
        .and_then(|ret| match ret {
            Type::Ident(ident) if ident == RustString => Some(quote!(#call.into_string())),
            Type::RustBox(_) => Some(quote!(::std::boxed::Box::from_raw(#call))),
            Type::UniquePtr(_) => Some(quote!(::cxx::UniquePtr::from_raw(#call))),
            Type::Ref(ty) => match &ty.inner {
                Type::Ident(ident) if ident == RustString => Some(quote!(#call.as_string())),
                _ => None,
            },
            Type::Str(_) => Some(quote!(#call.as_str())),
            _ => None,
        })
        .unwrap_or(call);
    quote! {
        #doc
        pub fn #ident(#(#args),*) #ret {
            extern "C" {
                #decl
            }
            unsafe {
                #setup
                #expr
            }
        }
    }
}

fn expand_rust_type(ety: &ExternType) -> TokenStream {
    let ident = &ety.ident;
    quote! {
        use super::#ident;
    }
}

fn expand_rust_function_shim(namespace: &Namespace, efn: &ExternFn, types: &Types) -> TokenStream {
    let ident = &efn.ident;
    let args = efn.args.iter().map(|arg| {
        let ident = &arg.ident;
        let ty = expand_extern_type(&arg.ty);
        if types.needs_indirect_abi(&arg.ty) {
            quote!(#ident: *mut #ty)
        } else {
            quote!(#ident: #ty)
        }
    });
    let vars = efn.args.iter().map(|arg| {
        let ident = &arg.ident;
        match &arg.ty {
            Type::Ident(i) if i == RustString => {
                quote!(::std::mem::take((*#ident).as_mut_string()))
            }
            Type::RustBox(_) => quote!(::std::boxed::Box::from_raw(#ident)),
            Type::UniquePtr(_) => quote!(::cxx::UniquePtr::from_raw(#ident)),
            Type::Ref(ty) => match &ty.inner {
                Type::Ident(i) if i == RustString => quote!(#ident.as_string()),
                _ => quote!(#ident),
            },
            Type::Str(_) => quote!(#ident.as_str()),
            ty if types.needs_indirect_abi(ty) => quote!(::std::ptr::read(#ident)),
            _ => quote!(#ident),
        }
    });
    let mut outparam = None;
    let call = quote! {
        ::cxx::private::catch_unwind(__fn, move || super::#ident(#(#vars),*))
    };
    let mut expr = efn
        .ret
        .as_ref()
        .and_then(|ret| match ret {
            Type::Ident(ident) if ident == RustString => {
                Some(quote!(::cxx::private::RustString::from(#call)))
            }
            Type::RustBox(_) => Some(quote!(::std::boxed::Box::into_raw(#call))),
            Type::UniquePtr(_) => Some(quote!(::cxx::UniquePtr::into_raw(#call))),
            Type::Ref(ty) => match &ty.inner {
                Type::Ident(ident) if ident == RustString => {
                    Some(quote!(::cxx::private::RustString::from_ref(#call)))
                }
                _ => None,
            },
            Type::Str(_) => Some(quote!(::cxx::private::RustStr::from(#call))),
            _ => None,
        })
        .unwrap_or(call);
    if indirect_return(&efn.ret, types) {
        let ret = expand_extern_type(efn.ret.as_ref().unwrap());
        outparam = Some(quote!(__return: *mut #ret));
        expr = quote!(::std::ptr::write(__return, #expr));
    }
    let ret = expand_extern_return_type(&efn.ret, types);
    let link_name = format!("{}cxxbridge02${}", namespace, ident);
    let local_name = format_ident!("__{}", ident);
    let catch_unwind_label = format!("::{}", ident);
    quote! {
        #[doc(hidden)]
        #[export_name = #link_name]
        unsafe extern "C" fn #local_name(#(#args,)* #outparam) #ret {
            let __fn = concat!(module_path!(), #catch_unwind_label);
            #expr
        }
    }
}

fn expand_rust_box(namespace: &Namespace, ident: &Ident) -> TokenStream {
    let link_prefix = format!("cxxbridge02$box${}{}$", namespace, ident);
    let link_uninit = format!("{}uninit", link_prefix);
    let link_drop = format!("{}drop", link_prefix);

    let local_prefix = format_ident!("{}__box_", ident);
    let local_uninit = format_ident!("{}uninit", local_prefix);
    let local_drop = format_ident!("{}drop", local_prefix);

    let span = ident.span();
    quote_spanned! {span=>
        #[doc(hidden)]
        #[export_name = #link_uninit]
        unsafe extern "C" fn #local_uninit(
            this: *mut ::std::boxed::Box<::std::mem::MaybeUninit<#ident>>,
        ) {
            ::std::ptr::write(
                this,
                ::std::boxed::Box::new(::std::mem::MaybeUninit::uninit()),
            );
        }
        #[doc(hidden)]
        #[export_name = #link_drop]
        unsafe extern "C" fn #local_drop(this: *mut ::std::boxed::Box<#ident>) {
            ::std::ptr::drop_in_place(this);
        }
    }
}

fn expand_rust_vec(namespace: &Namespace, ty: &Type, ident: &Ident) -> TokenStream {
    let inner = ty;
    let mangled = ty.to_mangled(&namespace.segments) + "$";
    let link_prefix = format!("cxxbridge02$rust_vec${}", mangled);
    let link_drop = format!("{}drop", link_prefix);
    let link_vector_from = format!("{}vector_from", link_prefix);
    let link_len = format!("{}len", link_prefix);

    let local_prefix = format_ident!("{}__vec_", ident);
    let local_drop = format_ident!("{}drop", local_prefix);
    let local_vector_from = format_ident!("{}vector_from", local_prefix);
    let local_len = format_ident!("{}len", local_prefix);

    let span = ty.span();
    quote_spanned! {span=>
        #[doc(hidden)]
        #[export_name = #link_drop]
        unsafe extern "C" fn #local_drop(this: *mut ::cxx::RustVec<#inner>) {
            std::ptr::drop_in_place(this);
        }
        #[export_name = #link_vector_from]
        unsafe extern "C" fn #local_vector_from(this: *mut ::cxx::RustVec<#inner>, vector: *mut ::cxx::RealVector<#inner>) {
            this.as_ref().unwrap().into_vector(vector.as_mut().unwrap());
        }
        #[export_name = #link_len]
        unsafe extern "C" fn #local_len(this: *const ::cxx::RustVec<#inner>) -> usize {
            this.as_ref().unwrap().len()
        }
    }
}

fn expand_unique_ptr(namespace: &Namespace, ty: &Type) -> TokenStream {
    let inner = ty;
    let mangled = ty.to_mangled(&namespace.segments) + "$";
    let prefix = format!("cxxbridge02$unique_ptr${}", mangled);
    let link_null = format!("{}null", prefix);
    let link_new = format!("{}new", prefix);
    let link_raw = format!("{}raw", prefix);
    let link_get = format!("{}get", prefix);
    let link_release = format!("{}release", prefix);
    let link_drop = format!("{}drop", prefix);

    quote! {
        unsafe impl ::cxx::private::UniquePtrTarget for #inner {
            fn __null() -> *mut ::std::ffi::c_void {
                extern "C" {
                    #[link_name = #link_null]
                    fn __null(this: *mut *mut ::std::ffi::c_void);
                }
                let mut repr = ::std::ptr::null_mut::<::std::ffi::c_void>();
                unsafe { __null(&mut repr) }
                repr
            }
            fn __new(mut value: Self) -> *mut ::std::ffi::c_void {
                extern "C" {
                    #[link_name = #link_new]
                    fn __new(this: *mut *mut ::std::ffi::c_void, value: *mut #inner);
                }
                let mut repr = ::std::ptr::null_mut::<::std::ffi::c_void>();
                unsafe { __new(&mut repr, &mut value) }
                repr
            }
            unsafe fn __raw(raw: *mut Self) -> *mut ::std::ffi::c_void {
                extern "C" {
                    #[link_name = #link_raw]
                    fn __raw(this: *mut *mut ::std::ffi::c_void, raw: *mut #inner);
                }
                let mut repr = ::std::ptr::null_mut::<::std::ffi::c_void>();
                __raw(&mut repr, raw);
                repr
            }
            unsafe fn __get(repr: *mut ::std::ffi::c_void) -> *const Self {
                extern "C" {
                    #[link_name = #link_get]
                    fn __get(this: *const *mut ::std::ffi::c_void) -> *const #inner;
                }
                __get(&repr)
            }
            unsafe fn __release(mut repr: *mut ::std::ffi::c_void) -> *mut Self {
                extern "C" {
                    #[link_name = #link_release]
                    fn __release(this: *mut *mut ::std::ffi::c_void) -> *mut #inner;
                }
                __release(&mut repr)
            }
            unsafe fn __drop(mut repr: *mut ::std::ffi::c_void) {
                extern "C" {
                    #[link_name = #link_drop]
                    fn __drop(this: *mut *mut ::std::ffi::c_void);
                }
                __drop(&mut repr);
            }
        }
    }
}

fn expand_vector(namespace: &Namespace, ty: &Type) -> TokenStream {
    let inner = ty;
    let mangled = ty.to_mangled(&namespace.segments) + "$";
    let prefix = format!("cxxbridge02$std$vector${}", mangled);
    let link_length = format!("{}length", prefix);
    let link_get_unchecked = format!("{}get_unchecked", prefix);
    let link_push_back = format!("{}push_back", prefix);

    quote! {
        impl ::cxx::private::VectorTarget<#inner> for #inner {
            fn get_unchecked(v: &::cxx::RealVector<#inner>, pos: usize) -> &#inner {
                extern "C" {
                    #[link_name = #link_get_unchecked]
                    fn __get_unchecked(_: &::cxx::RealVector<#inner>, _: usize) -> &#inner;
                }
                unsafe {
                    __get_unchecked(v, pos)
                }
            }
            fn vector_length(v: &::cxx::RealVector<#inner>) -> usize {
                unsafe {
                    extern "C" {
                        #[link_name = #link_length]
                        fn __vector_length(_: &::cxx::RealVector<#inner>) -> usize;
                    }
                    __vector_length(v)
                }
            }
            fn push_back(v: &::cxx::RealVector<#inner>, item: &#inner) {
                unsafe {
                    extern "C" {
                        #[link_name = #link_push_back]
                        fn __push_back(_: &::cxx::RealVector<#inner>, _: &#inner) -> usize;
                    }
                    __push_back(v, item);
                }
            }
        }
    }
}

fn expand_return_type(ret: &Option<Type>) -> TokenStream {
    match ret {
        Some(ret) => quote!(-> #ret),
        None => TokenStream::new(),
    }
}

fn indirect_return(ret: &Option<Type>, types: &Types) -> bool {
    ret.as_ref()
        .map_or(false, |ret| types.needs_indirect_abi(ret))
}

fn expand_extern_type(ty: &Type) -> TokenStream {
    match ty {
        Type::Ident(ident) if ident == RustString => quote!(::cxx::private::RustString),
        Type::RustBox(ty) | Type::UniquePtr(ty) => {
            let inner = expand_extern_type(&ty.inner);
            quote!(*mut #inner)
        }
        Type::RustVec(ty) => quote!(::cxx::RustVec<#ty>),
        Type::Ref(ty) => match &ty.inner {
            Type::Ident(ident) if ident == RustString => quote!(&::cxx::private::RustString),
            Type::RustVec(ty) => {
                let inner = expand_extern_type(&ty.inner);
                quote!(&::cxx::RustVec<#inner>)
            }
            _ => quote!(#ty),
        },
        Type::Str(_) => quote!(::cxx::private::RustStr),
        _ => quote!(#ty),
    }
}

fn expand_extern_return_type(ret: &Option<Type>, types: &Types) -> TokenStream {
    let ret = match ret {
        Some(ret) if !types.needs_indirect_abi(ret) => ret,
        _ => return TokenStream::new(),
    };
    let ty = expand_extern_type(ret);
    quote!(-> #ty)
}
