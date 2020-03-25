use crate::syntax::{
    attrs, error, Api, Atom, Doc, ExternFn, ExternType, Lang, Receiver, Ref, Signature, Struct,
    Ty1, Type, Var,
};
use proc_macro2::Ident;
use quote::{format_ident, quote};
use syn::{
    Abi, Error, Fields, FnArg, ForeignItem, ForeignItemFn, ForeignItemType, GenericArgument, Item,
    ItemForeignMod, ItemStruct, Pat, PathArguments, Result, ReturnType, Type as RustType,
    TypeBareFn, TypePath, TypeReference,
};

pub fn parse_items(items: Vec<Item>) -> Result<Vec<Api>> {
    let mut apis = Vec::new();
    for item in items {
        match item {
            Item::Struct(item) => {
                let strct = parse_struct(item)?;
                apis.push(strct);
            }
            Item::ForeignMod(foreign_mod) => {
                let functions = parse_foreign_mod(foreign_mod)?;
                apis.extend(functions);
            }
            Item::Use(item) => return Err(Error::new_spanned(item, error::USE_NOT_ALLOWED)),
            _ => return Err(Error::new_spanned(item, "unsupported item")),
        }
    }
    Ok(apis)
}

fn parse_struct(item: ItemStruct) -> Result<Api> {
    let generics = &item.generics;
    if !generics.params.is_empty() || generics.where_clause.is_some() {
        let struct_token = item.struct_token;
        let ident = &item.ident;
        let where_clause = &generics.where_clause;
        let span = quote!(#struct_token #ident #generics #where_clause);
        return Err(Error::new_spanned(
            span,
            "struct with generic parameters is not supported yet",
        ));
    }

    let mut doc = Doc::new();
    let mut derives = Vec::new();
    attrs::parse(&item.attrs, &mut doc, Some(&mut derives))?;
    check_reserved_name(&item.ident)?;

    let fields = match item.fields {
        Fields::Named(fields) => fields,
        Fields::Unit => return Err(Error::new_spanned(item, "unit structs are not supported")),
        Fields::Unnamed(_) => {
            return Err(Error::new_spanned(item, "tuple structs are not supported"))
        }
    };

    Ok(Api::Struct(Struct {
        doc,
        derives,
        struct_token: item.struct_token,
        ident: item.ident,
        brace_token: fields.brace_token,
        fields: fields
            .named
            .into_iter()
            .map(|field| {
                Ok(Var {
                    ident: field.ident.unwrap(),
                    ty: parse_type(&field.ty)?,
                })
            })
            .collect::<Result<_>>()?,
    }))
}

fn parse_foreign_mod(foreign_mod: ItemForeignMod) -> Result<Vec<Api>> {
    let lang = parse_lang(foreign_mod.abi)?;
    let api_type = match lang {
        Lang::Cxx => Api::CxxType,
        Lang::Rust => Api::RustType,
    };
    let api_function = match lang {
        Lang::Cxx => Api::CxxFunction,
        Lang::Rust => Api::RustFunction,
    };

    let mut items = Vec::new();
    for foreign in &foreign_mod.items {
        match foreign {
            ForeignItem::Type(foreign) => {
                check_reserved_name(&foreign.ident)?;
                let ety = parse_extern_type(foreign)?;
                items.push(api_type(ety));
            }
            ForeignItem::Fn(foreign) => {
                let efn = parse_extern_fn(foreign, lang)?;
                items.push(api_function(efn));
            }
            ForeignItem::Macro(foreign) if foreign.mac.path.is_ident("include") => {
                let include = foreign.mac.parse_body()?;
                items.push(Api::Include(include));
            }
            _ => return Err(Error::new_spanned(foreign, "unsupported foreign item")),
        }
    }
    Ok(items)
}

fn parse_lang(abi: Abi) -> Result<Lang> {
    let name = match &abi.name {
        Some(name) => name,
        None => {
            return Err(Error::new_spanned(
                abi,
                "ABI name is required, extern \"C\" or extern \"Rust\"",
            ));
        }
    };
    match name.value().as_str() {
        "C" | "C++" => Ok(Lang::Cxx),
        "Rust" => Ok(Lang::Rust),
        _ => Err(Error::new_spanned(abi, "unrecognized ABI")),
    }
}

fn parse_extern_type(foreign_type: &ForeignItemType) -> Result<ExternType> {
    let doc = attrs::parse_doc(&foreign_type.attrs)?;
    let type_token = foreign_type.type_token;
    let ident = foreign_type.ident.clone();
    Ok(ExternType {
        doc,
        type_token,
        ident,
    })
}

fn parse_extern_fn(foreign_fn: &ForeignItemFn, lang: Lang) -> Result<ExternFn> {
    let generics = &foreign_fn.sig.generics;
    if !generics.params.is_empty() || generics.where_clause.is_some() {
        return Err(Error::new_spanned(
            foreign_fn,
            "extern function with generic parameters is not supported yet",
        ));
    }
    if let Some(variadic) = &foreign_fn.sig.variadic {
        return Err(Error::new_spanned(
            variadic,
            "variadic function is not supported yet",
        ));
    }

    let mut receiver = None;
    let mut args = Vec::new();
    for arg in &foreign_fn.sig.inputs {
        match arg {
            FnArg::Receiver(receiver) => {
                return Err(Error::new_spanned(receiver, "unsupported signature"))
            }
            FnArg::Typed(arg) => {
                let ident = match arg.pat.as_ref() {
                    Pat::Ident(pat) => pat.ident.clone(),
                    _ => return Err(Error::new_spanned(arg, "unsupported signature")),
                };
                let ty = parse_type(&arg.ty)?;
                if ident != "self" {
                    args.push(Var { ident, ty });
                    continue;
                }
                if let Type::Ref(reference) = ty {
                    if let Type::Ident(ident) = reference.inner {
                        receiver = Some(Receiver {
                            mutability: reference.mutability,
                            ident,
                        });
                        continue;
                    }
                }
                return Err(Error::new_spanned(arg, "unsupported method receiver"));
            }
        }
    }

    let mut throws = false;
    let ret = parse_return_type(&foreign_fn.sig.output, &mut throws)?;
    let doc = attrs::parse_doc(&foreign_fn.attrs)?;
    let fn_token = foreign_fn.sig.fn_token;
    let ident = foreign_fn.sig.ident.clone();
    let mut foreign_fn2 = foreign_fn.clone();
    foreign_fn2.attrs.clear();
    let tokens = quote!(#foreign_fn2);
    let semi_token = foreign_fn.semi_token;

    Ok(ExternFn {
        lang,
        doc,
        ident,
        sig: Signature {
            fn_token,
            receiver,
            args,
            ret,
            throws,
            tokens,
        },
        semi_token,
    })
}

fn parse_type(ty: &RustType) -> Result<Type> {
    match ty {
        RustType::Reference(ty) => parse_type_reference(ty),
        RustType::Path(ty) => parse_type_path(ty),
        RustType::BareFn(ty) => parse_type_fn(ty),
        RustType::Tuple(ty) if ty.elems.is_empty() => Ok(Type::Void(ty.paren_token.span)),
        _ => Err(Error::new_spanned(ty, "unsupported type")),
    }
}

fn parse_type_reference(ty: &TypeReference) -> Result<Type> {
    let inner = parse_type(&ty.elem)?;
    let which = match &inner {
        Type::Ident(ident) if ident == "str" => {
            if ty.mutability.is_some() {
                return Err(Error::new_spanned(ty, "unsupported type"));
            } else {
                Type::Str
            }
        }
        _ => Type::Ref,
    };
    Ok(which(Box::new(Ref {
        ampersand: ty.and_token,
        mutability: ty.mutability,
        inner,
    })))
}

fn parse_type_path(ty: &TypePath) -> Result<Type> {
    let path = &ty.path;
    if ty.qself.is_none() && path.leading_colon.is_none() && path.segments.len() == 1 {
        let segment = &path.segments[0];
        let ident = segment.ident.clone();
        match &segment.arguments {
            PathArguments::None => return Ok(Type::Ident(ident)),
            PathArguments::AngleBracketed(generic) => {
                if ident == "UniquePtr" && generic.args.len() == 1 {
                    if let GenericArgument::Type(arg) = &generic.args[0] {
                        let inner = parse_type(arg)?;
                        return Ok(Type::UniquePtr(Box::new(Ty1 {
                            name: ident,
                            langle: generic.lt_token,
                            inner,
                            rangle: generic.gt_token,
                        })));
                    }
                } else if ident == "Vector" && generic.args.len() == 1 {
                    if let GenericArgument::Type(arg) = &generic.args[0] {
                        let inner = parse_type(arg)?;
                        return Ok(Type::Vector(Box::new(Ty1 {
                            name: ident,
                            langle: generic.lt_token,
                            inner,
                            rangle: generic.gt_token,
                        })));
                    }
                } else if ident == "Box" && generic.args.len() == 1 {
                    if let GenericArgument::Type(arg) = &generic.args[0] {
                        let inner = parse_type(arg)?;
                        return Ok(Type::RustBox(Box::new(Ty1 {
                            name: ident,
                            langle: generic.lt_token,
                            inner,
                            rangle: generic.gt_token,
                        })));
                    }
                } else if ident == "Vec" && generic.args.len() == 1 {
                    if let GenericArgument::Type(arg) = &generic.args[0] {
                        let inner = parse_type(arg)?;
                        return Ok(Type::RustVec(Box::new(Ty1 {
                            name: ident,
                            langle: generic.lt_token,
                            inner,
                            rangle: generic.gt_token,
                        })));
                    }
                }
            }
            PathArguments::Parenthesized(_) => {}
        }
    }
    Err(Error::new_spanned(ty, "unsupported type"))
}

fn parse_type_fn(ty: &TypeBareFn) -> Result<Type> {
    if ty.lifetimes.is_some() {
        return Err(Error::new_spanned(
            ty,
            "function pointer with lifetime parameters is not supported yet",
        ));
    }
    if ty.variadic.is_some() {
        return Err(Error::new_spanned(
            ty,
            "variadic function pointer is not supported yet",
        ));
    }
    let args = ty
        .inputs
        .iter()
        .enumerate()
        .map(|(i, arg)| {
            let ty = parse_type(&arg.ty)?;
            let ident = match &arg.name {
                Some(ident) => ident.0.clone(),
                None => format_ident!("_{}", i),
            };
            Ok(Var { ident, ty })
        })
        .collect::<Result<_>>()?;
    let mut throws = false;
    let ret = parse_return_type(&ty.output, &mut throws)?;
    let tokens = quote!(#ty);
    Ok(Type::Fn(Box::new(Signature {
        fn_token: ty.fn_token,
        receiver: None,
        args,
        ret,
        throws,
        tokens,
    })))
}

fn parse_return_type(ty: &ReturnType, throws: &mut bool) -> Result<Option<Type>> {
    let mut ret = match ty {
        ReturnType::Default => return Ok(None),
        ReturnType::Type(_, ret) => ret.as_ref(),
    };
    if let RustType::Path(ty) = ret {
        let path = &ty.path;
        if ty.qself.is_none() && path.leading_colon.is_none() && path.segments.len() == 1 {
            let segment = &path.segments[0];
            let ident = segment.ident.clone();
            if let PathArguments::AngleBracketed(generic) = &segment.arguments {
                if ident == "Result" && generic.args.len() == 1 {
                    if let GenericArgument::Type(arg) = &generic.args[0] {
                        ret = arg;
                        *throws = true;
                    }
                }
            }
        }
    }
    match parse_type(ret)? {
        Type::Void(_) => Ok(None),
        ty => Ok(Some(ty)),
    }
}

fn check_reserved_name(ident: &Ident) -> Result<()> {
    if ident == "Box" || ident == "UniquePtr" || ident == "Vector" || Atom::from(ident).is_some() {
        Err(Error::new(ident.span(), "reserved name"))
    } else {
        Ok(())
    }
}
