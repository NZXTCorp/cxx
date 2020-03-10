use crate::gen::include;
use crate::gen::out::OutFile;
use crate::syntax::atom::Atom::{self, *};
use crate::syntax::mangled::ToMangled;
use crate::syntax::typename::ToTypename;
use crate::syntax::{Api, ExternFn, Struct, Type, Types, Var};
use proc_macro2::Ident;

pub(super) fn gen(namespace: Vec<String>, apis: &[Api], types: &Types, header: bool) -> OutFile {
    let mut out_file = OutFile::new(namespace.clone(), header);
    let out = &mut out_file;

    if header {
        writeln!(out, "#pragma once");
    }

    for api in apis {
        if let Api::Include(include) = api {
            out.include.insert(include.value());
        }
    }

    write_includes(out, types);
    write_include_cxxbridge(out, apis, types);

    out.next_section();
    for name in &namespace {
        writeln!(out, "namespace {} {{", name);
    }

    out.next_section();
    for api in apis {
        match api {
            Api::Struct(strct) => write_struct_decl(out, &strct.ident),
            Api::CxxType(ety) => write_struct_using(out, &ety.ident),
            Api::RustType(ety) => write_struct_decl(out, &ety.ident),
            _ => (),
        }
    }

    for api in apis {
        if let Api::Struct(strct) = api {
            out.next_section();
            write_struct(out, strct);
        }
    }

    if !header {
        out.begin_block("extern \"C\"");
        for api in apis {
            let (efn, write): (_, fn(_, _, _)) = match api {
                Api::CxxFunction(efn) => (efn, write_cxx_function_shim),
                Api::RustFunction(efn) => (efn, write_rust_function_decl),
                _ => continue,
            };
            out.next_section();
            write(out, efn, types);
        }
        out.end_block("extern \"C\"");
    }

    for api in apis {
        if let Api::RustFunction(efn) = api {
            out.next_section();
            write_rust_function_shim(out, efn, types);
        }
    }

    out.next_section();
    for name in namespace.iter().rev() {
        writeln!(out, "}} // namespace {}", name);
    }

    if !header {
        out.next_section();
        write_generic_instantiations(out, types);
    }

    out.prepend(out.include.to_string());

    out_file
}

fn write_includes(out: &mut OutFile, types: &Types) {
    for ty in types {
        match ty {
            Type::Ident(ident) => match Atom::from(ident) {
                Some(U8) | Some(U16) | Some(U32) | Some(U64) | Some(Usize) | Some(I8)
                | Some(I16) | Some(I32) | Some(I64) | Some(Isize) => out.include.cstdint = true,
                Some(CxxString) => out.include.string = true,
                Some(Bool) | Some(RustString) | Some(F32) | Some(F64) | None => (),
            },
            Type::RustBox(_) => out.include.type_traits = true,
            Type::UniquePtr(_) => out.include.memory = true,
            Type::Vector(_) => out.include.vector = true,
            _ => (),
        }
    }
}

fn write_include_cxxbridge(out: &mut OutFile, apis: &[Api], types: &Types) {
    let mut needs_rust_box = false;
    let mut needs_rust_vec = false;
    for ty in types {
        if let Type::RustBox(_) = ty {
            needs_rust_box = true;
            break;
        } else if let Type::RustVec(_) = ty {
            needs_rust_vec = true;
            break;
        }
    }

    let mut needs_manually_drop = false;
    let mut needs_maybe_uninit = false;
    for api in apis {
        if let Api::RustFunction(efn) = api {
            for arg in &efn.args {
                if arg.ty != RustString && types.needs_indirect_abi(&arg.ty) {
                    needs_manually_drop = true;
                    break;
                }
            }
            if let Some(ret) = &efn.ret {
                if types.needs_indirect_abi(ret) {
                    needs_maybe_uninit = true;
                }
            }
        }
    }

    out.begin_block("namespace rust");
    out.begin_block("inline namespace cxxbridge01");

    if needs_rust_box || needs_rust_vec || needs_manually_drop || needs_maybe_uninit {
        writeln!(out, "// #include \"cxxbridge.h\"");
    }

    if needs_rust_box {
        out.next_section();
        for line in include::get("CXXBRIDGE01_RUST_BOX").lines() {
            if !line.trim_start().starts_with("//") {
                writeln!(out, "{}", line);
            }
        }
    }
    if needs_rust_vec {
        out.next_section();
        for line in include::get("CXXBRIDGE01_RUST_VEC").lines() {
            if !line.trim_start().starts_with("//") {
                writeln!(out, "{}", line);
            }
        }
    }

    if needs_manually_drop {
        out.next_section();
        writeln!(out, "template <typename T>");
        writeln!(out, "union ManuallyDrop {{");
        writeln!(out, "  T value;");
        writeln!(
            out,
            "  ManuallyDrop(T &&value) : value(::std::move(value)) {{}}",
        );
        writeln!(out, "  ~ManuallyDrop() {{}}");
        writeln!(out, "}};");
    }

    if needs_maybe_uninit {
        out.next_section();
        writeln!(out, "template <typename T>");
        writeln!(out, "union MaybeUninit {{");
        writeln!(out, "  T value;");
        writeln!(out, "  MaybeUninit() {{}}");
        writeln!(out, "  ~MaybeUninit() {{}}");
        writeln!(out, "}};");
    }

    out.end_block("namespace cxxbridge01");
    out.end_block("namespace rust");
}

fn write_struct(out: &mut OutFile, strct: &Struct) {
    for line in strct.doc.to_string().lines() {
        writeln!(out, "//{}", line);
    }
    writeln!(out, "struct {} final {{", strct.ident);
    for field in &strct.fields {
        write!(out, "  ");
        write_type_space(out, &field.ty);
        writeln!(out, "{};", field.ident);
    }
    writeln!(out, "}};");
}

fn write_struct_decl(out: &mut OutFile, ident: &Ident) {
    writeln!(out, "struct {};", ident);
}

fn write_struct_using(out: &mut OutFile, ident: &Ident) {
    writeln!(out, "using {} = {};", ident, ident);
}

fn write_cxx_function_shim(out: &mut OutFile, efn: &ExternFn, types: &Types) {
    let indirect_return = efn
        .ret
        .as_ref()
        .map_or(false, |ret| types.needs_indirect_abi(ret));
    write_extern_return_type(out, &efn.ret, types);
    for name in out.namespace.clone() {
        write!(out, "{}$", name);
    }
    write!(out, "cxxbridge01${}(", efn.ident);
    for (i, arg) in efn.args.iter().enumerate() {
        if i > 0 {
            write!(out, ", ");
        }
        if arg.ty == RustString {
            write!(out, "const ");
        }
        write_extern_arg(out, arg, types);
    }
    if indirect_return {
        if !efn.args.is_empty() {
            write!(out, ", ");
        }
        write_return_type(out, &efn.ret);
        write!(out, "*return$");
    }
    writeln!(out, ") noexcept {{");
    write!(out, "  ");
    write_return_type(out, &efn.ret);
    write!(out, "(*{}$)(", efn.ident);
    for (i, arg) in efn.args.iter().enumerate() {
        if i > 0 {
            write!(out, ", ");
        }
        write_type(out, &arg.ty);
    }
    writeln!(out, ") = {};", efn.ident);
    write!(out, "  ");
    if indirect_return {
        write!(out, "new (return$) ");
        write_type(out, efn.ret.as_ref().unwrap());
        write!(out, "(");
    } else if let Some(ret) = &efn.ret {
        write!(out, "return ");
        match ret {
            Type::Ref(_) => write!(out, "&"),
            Type::Str(_) => write!(out, "::rust::Str::Repr("),
            _ => {}
        }
    }
    write!(out, "{}$(", efn.ident);
    for (i, arg) in efn.args.iter().enumerate() {
        if i > 0 {
            write!(out, ", ");
        }
        if let Type::RustBox(_) = &arg.ty {
            write_type(out, &arg.ty);
            write!(out, "::from_raw({})", arg.ident);
        } else if let Type::UniquePtr(_) = &arg.ty {
            write_type(out, &arg.ty);
            write!(out, "({})", arg.ident);
        } else if arg.ty == RustString {
            write!(
                out,
                "::rust::String(::rust::unsafe_bitcopy, *{})",
                arg.ident,
            );
        } else if types.needs_indirect_abi(&arg.ty) {
            write!(out, "::std::move(*{})", arg.ident);
        } else {
            write!(out, "{}", arg.ident);
        }
    }
    write!(out, ")");
    match &efn.ret {
        Some(Type::RustBox(_)) => write!(out, ".into_raw()"),
        Some(Type::UniquePtr(_)) => write!(out, ".release()"),
        Some(Type::Vector(_)) => write!(
            out,
            " /* Use RVO to convert to r-value and move construct */"
        ),
        Some(Type::Str(_)) => write!(out, ")"),
        _ => {}
    }
    if indirect_return {
        write!(out, ")");
    }
    writeln!(out, ";");
    writeln!(out, "}}");
}

fn write_rust_function_decl(out: &mut OutFile, efn: &ExternFn, types: &Types) {
    write_extern_return_type(out, &efn.ret, types);
    for name in out.namespace.clone() {
        write!(out, "{}$", name);
    }
    write!(out, "cxxbridge01${}(", efn.ident);
    for (i, arg) in efn.args.iter().enumerate() {
        if i > 0 {
            write!(out, ", ");
        }
        write_extern_arg(out, arg, types);
    }
    if efn
        .ret
        .as_ref()
        .map_or(false, |ret| types.needs_indirect_abi(ret))
    {
        if !efn.args.is_empty() {
            write!(out, ", ");
        }
        write_return_type(out, &efn.ret);
        write!(out, "*return$");
    }
    writeln!(out, ") noexcept;");
}

fn write_rust_function_shim(out: &mut OutFile, efn: &ExternFn, types: &Types) {
    let indirect_return = efn
        .ret
        .as_ref()
        .map_or(false, |ret| types.needs_indirect_abi(ret));
    for line in efn.doc.to_string().lines() {
        writeln!(out, "//{}", line);
    }
    write_return_type(out, &efn.ret);
    write!(out, "{}(", efn.ident);
    for (i, arg) in efn.args.iter().enumerate() {
        if i > 0 {
            write!(out, ", ");
        }
        write_type_space(out, &arg.ty);
        write!(out, "{}", arg.ident);
    }
    write!(out, ") noexcept");
    if out.header {
        writeln!(out, ";");
    } else {
        writeln!(out, " {{");
        for arg in &efn.args {
            if arg.ty != RustString && types.needs_indirect_abi(&arg.ty) {
                write!(out, "  ::rust::ManuallyDrop<");
                write_type(out, &arg.ty);
                writeln!(out, "> {}$(::std::move({0}));", arg.ident);
            }
        }
        write!(out, "  ");
        if indirect_return {
            write!(out, "::rust::MaybeUninit<");
            write_type(out, efn.ret.as_ref().unwrap());
            writeln!(out, "> return$;");
            write!(out, "  ");
        } else if let Some(ret) = &efn.ret {
            write!(out, "return ");
            match ret {
                Type::RustBox(_) => {
                    write_type(out, ret);
                    write!(out, "::from_raw(");
                }
                Type::UniquePtr(_) => {
                    write_type(out, ret);
                    write!(out, "(");
                }
                Type::Ref(_) => write!(out, "*"),
                _ => {}
            }
        }
        for name in out.namespace.clone() {
            write!(out, "{}$", name);
        }
        write!(out, "cxxbridge01${}(", efn.ident);
        for (i, arg) in efn.args.iter().enumerate() {
            if i > 0 {
                write!(out, ", ");
            }
            match &arg.ty {
                Type::Str(_) => write!(out, "::rust::Str::Repr("),
                ty if types.needs_indirect_abi(ty) => write!(out, "&"),
                _ => {}
            }
            write!(out, "{}", arg.ident);
            match &arg.ty {
                Type::RustBox(_) => write!(out, ".into_raw()"),
                Type::UniquePtr(_) => write!(out, ".release()"),
                Type::Str(_) => write!(out, ")"),
                ty if ty != RustString && types.needs_indirect_abi(ty) => write!(out, "$.value"),
                _ => {}
            }
        }
        if indirect_return {
            if !efn.args.is_empty() {
                write!(out, ", ");
            }
            write!(out, "&return$.value");
        }
        write!(out, ")");
        if let Some(ret) = &efn.ret {
            if let Type::RustBox(_) | Type::UniquePtr(_) = ret {
                write!(out, ")");
            }
        }
        writeln!(out, ";");
        if indirect_return {
            writeln!(out, "  return ::std::move(return$.value);");
        }
        writeln!(out, "}}");
    }
}

fn write_return_type(out: &mut OutFile, ty: &Option<Type>) {
    match ty {
        None => write!(out, "void "),
        Some(ty) => write_type_space(out, ty),
    }
}

fn write_extern_return_type(out: &mut OutFile, ty: &Option<Type>, types: &Types) {
    match ty {
        Some(Type::RustBox(ty)) | Some(Type::UniquePtr(ty)) => {
            write_type_space(out, &ty.inner);
            write!(out, "*");
        }
        Some(Type::Ref(ty)) => {
            if ty.mutability.is_none() {
                write!(out, "const ");
            }
            write_type(out, &ty.inner);
            write!(out, " *");
        }
        Some(Type::Str(_)) => write!(out, "::rust::Str::Repr "),
        Some(ty) if types.needs_indirect_abi(ty) => write!(out, "void "),
        _ => write_return_type(out, ty),
    }
}

fn write_extern_arg(out: &mut OutFile, arg: &Var, types: &Types) {
    match &arg.ty {
        Type::RustBox(ty) | Type::UniquePtr(ty) | Type::Vector(ty) => {
            write_type_space(out, &ty.inner);
            write!(out, "*");
        }
        Type::Str(_) => write!(out, "::rust::Str::Repr "),
        _ => write_type_space(out, &arg.ty),
    }
    if types.needs_indirect_abi(&arg.ty) {
        write!(out, "*");
    }
    write!(out, "{}", arg.ident);
}

fn write_type(out: &mut OutFile, ty: &Type) {
    match ty {
        Type::Ident(ident) => match Atom::from(ident) {
            Some(a) => write!(out, "{}", a.to_cxx()),
            None => write!(out, "{}", ident),
        },
        Type::RustBox(ty) => {
            write!(out, "::rust::Box<");
            write_type(out, &ty.inner);
            write!(out, ">");
        }
        Type::RustVec(ty) => {
            write!(out, "cxxbridge::RustVec<");
            write_type(out, &ty.inner);
            write!(out, ">");
        }
        Type::UniquePtr(ptr) => {
            write!(out, "::std::unique_ptr<");
            write_type(out, &ptr.inner);
            write!(out, ">");
        }
        Type::Vector(ty) => {
            write!(out, "std::vector<");
            write_type(out, &ty.inner);
            write!(out, ">");
        }
        Type::Ref(r) => {
            if r.mutability.is_none() {
                write!(out, "const ");
            }
            write_type(out, &r.inner);
            write!(out, " &");
        }
        Type::Str(_) => {
            write!(out, "::rust::Str");
        }
    }
}

fn write_type_space(out: &mut OutFile, ty: &Type) {
    write_type(out, ty);
    match ty {
        Type::Ident(_)
        | Type::RustBox(_)
        | Type::UniquePtr(_)
        | Type::Str(_)
        | Type::Vector(_)
        | Type::RustVec(_) => write!(out, " "),
        Type::Ref(_) => {}
    }
}

fn write_generic_instantiations(out: &mut OutFile, types: &Types) {
    fn allow_unique_ptr(ident: &Ident) -> bool {
        Atom::from(ident).is_none()
    }

    fn allow_vector(ident: &Ident) -> bool {
        // For now, allow either u8 or user type
        if let Some(ty) = Atom::from(ident) {
            if ty == Atom::U8 {
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    out.begin_block("extern \"C\"");
    for ty in types {
        if let Type::RustBox(ty) = ty {
            if let Type::Ident(inner) = &ty.inner {
                out.next_section();
                write_rust_box_extern(out, inner);
            }
        } else if let Type::RustVec(ty) = ty {
            if let Type::Ident(_) = &ty.inner {
                out.next_section();
                write_rust_vec_extern(out, &ty.inner);
            }
        } else if let Type::UniquePtr(ptr) = ty {
            if let Type::Ident(inner) = &ptr.inner {
                if allow_unique_ptr(inner) {
                    out.next_section();
                    write_unique_ptr(out, &ptr.inner);
                }
            } else if let Type::Vector(_) = &ptr.inner {
                out.next_section();
                write_unique_ptr(out, &ptr.inner);
            }
        } else if let Type::Vector(ptr) = ty {
            if let Type::Ident(inner) = &ptr.inner {
                if allow_vector(inner) {
                    out.next_section();
                    write_vector(out, inner);
                }
            }
        }
    }
    out.end_block("extern \"C\"");

    out.begin_block("namespace rust");
    out.begin_block("inline namespace cxxbridge01");
    for ty in types {
        if let Type::RustBox(ty) = ty {
            if let Type::Ident(inner) = &ty.inner {
                write_rust_box_impl(out, inner);
            }
        } else if let Type::RustVec(ty) = ty {
            if let Type::Ident(_) = &ty.inner {
                write_rust_vec_impl(out, &ty.inner);
            }
        }
    }
    out.end_block("namespace cxxbridge01");
    out.end_block("namespace rust");
}

fn write_rust_box_extern(out: &mut OutFile, ident: &Ident) {
    let mut inner = String::new();
    for name in &out.namespace {
        inner += name;
        inner += "::";
    }
    inner += &ident.to_string();
    let instance = inner.replace("::", "$");

    writeln!(out, "#ifndef CXXBRIDGE01_RUST_BOX_{}", instance);
    writeln!(out, "#define CXXBRIDGE01_RUST_BOX_{}", instance);
    writeln!(
        out,
        "void cxxbridge01$box${}$uninit(::rust::Box<{}> *ptr) noexcept;",
        instance, inner,
    );
    writeln!(
        out,
        "void cxxbridge01$box${}$drop(::rust::Box<{}> *ptr) noexcept;",
        instance, inner,
    );
    writeln!(out, "#endif // CXXBRIDGE01_RUST_BOX_{}", instance);
}

fn write_rust_vec_extern(out: &mut OutFile, ty: &Type) {
    let inner = ty.to_typename(&out.namespace);
    let instance = ty.to_mangled(&out.namespace);

    writeln!(out, "#ifndef CXXBRIDGE01_RUST_VEC_{}", instance);
    writeln!(out, "#define CXXBRIDGE01_RUST_VEC_{}", instance);
    writeln!(
        out,
        "void cxxbridge01$rust_vec${}$drop(cxxbridge::RustVec<{}> *ptr) noexcept;",
        instance, inner,
    );
    writeln!(
        out,
        "void cxxbridge01$rust_vec${}$to_vector(const cxxbridge::RustVec<{}> *ptr, const std::vector<{}> &vector) noexcept;",
        instance, inner, inner
    );
    writeln!(out, "#endif // CXXBRIDGE01_RUST_VEC_{}", instance);
}

fn write_rust_box_impl(out: &mut OutFile, ident: &Ident) {
    let mut inner = String::new();
    for name in &out.namespace {
        inner += name;
        inner += "::";
    }
    inner += &ident.to_string();
    let instance = inner.replace("::", "$");

    writeln!(out, "template <>");
    writeln!(out, "void Box<{}>::uninit() noexcept {{", inner);
    writeln!(out, "  return cxxbridge01$box${}$uninit(this);", instance);
    writeln!(out, "}}");

    writeln!(out, "template <>");
    writeln!(out, "void Box<{}>::drop() noexcept {{", inner);
    writeln!(out, "  return cxxbridge01$box${}$drop(this);", instance);
    writeln!(out, "}}");
}

fn write_rust_vec_impl(out: &mut OutFile, ty: &Type) {
    let inner = ty.to_typename(&out.namespace);
    let instance = ty.to_mangled(&out.namespace);

    writeln!(out, "template <>");
    writeln!(out, "void RustVec<{}>::drop() noexcept {{", inner);
    writeln!(
        out,
        "  return cxxbridge01$rust_vec${}$drop(this);",
        instance
    );
    writeln!(out, "}}");

    writeln!(out, "template <>");
    writeln!(
        out,
        "void RustVec<{}>::to_vector(const std::vector<{}>& vector) const noexcept {{",
        inner, inner
    );
    writeln!(
        out,
        "  return cxxbridge01$rust_vec${}$to_vector(this, vector);",
        instance
    );
    writeln!(out, "}}");
}

fn write_unique_ptr(out: &mut OutFile, ty: &Type) {
    let inner = ty.to_typename(&out.namespace);
    let instance = ty.to_mangled(&out.namespace);

    writeln!(out, "#ifndef CXXBRIDGE01_UNIQUE_PTR_{}", instance);
    writeln!(out, "#define CXXBRIDGE01_UNIQUE_PTR_{}", instance);
    writeln!(
        out,
        "static_assert(sizeof(::std::unique_ptr<{}>) == sizeof(void *), \"\");",
        inner,
    );
    writeln!(
        out,
        "static_assert(alignof(::std::unique_ptr<{}>) == alignof(void *), \"\");",
        inner,
    );
    writeln!(
        out,
        "void cxxbridge01$unique_ptr${}$null(::std::unique_ptr<{}> *ptr) noexcept {{",
        instance, inner,
    );
    writeln!(out, "  new (ptr) ::std::unique_ptr<{}>();", inner);
    writeln!(out, "}}");
    writeln!(
        out,
        "void cxxbridge01$unique_ptr${}$new(::std::unique_ptr<{}> *ptr, {} *value) noexcept {{",
        instance, inner, inner,
    );
    writeln!(
        out,
        "  new (ptr) ::std::unique_ptr<{}>(new {}(::std::move(*value)));",
        inner, inner,
    );
    writeln!(out, "}}");
    writeln!(
        out,
        "void cxxbridge01$unique_ptr${}$raw(::std::unique_ptr<{}> *ptr, {} *raw) noexcept {{",
        instance, inner, inner,
    );
    writeln!(out, "  new (ptr) ::std::unique_ptr<{}>(raw);", inner);
    writeln!(out, "}}");
    writeln!(
        out,
        "const {} *cxxbridge01$unique_ptr${}$get(const ::std::unique_ptr<{}>& ptr) noexcept {{",
        inner, instance, inner,
    );
    writeln!(out, "  return ptr.get();");
    writeln!(out, "}}");
    writeln!(
        out,
        "{} *cxxbridge01$unique_ptr${}$release(::std::unique_ptr<{}>& ptr) noexcept {{",
        inner, instance, inner,
    );
    writeln!(out, "  return ptr.release();");
    writeln!(out, "}}");
    writeln!(
        out,
        "void cxxbridge01$unique_ptr${}$drop(::std::unique_ptr<{}> *ptr) noexcept {{",
        instance, inner,
    );
    writeln!(out, "  ptr->~unique_ptr();");
    writeln!(out, "}}");
    writeln!(out, "#endif // CXXBRIDGE01_UNIQUE_PTR_{}", instance);
}

fn write_vector(out: &mut OutFile, ident: &Ident) {
    let mut inner = String::new();
    // Do not apply namespace to built-in type
    let is_user_type = Atom::from(ident).is_none();
    if is_user_type {
        for name in &out.namespace {
            inner += name;
            inner += "::";
        }
    }
    let mut instance = inner.clone();
    if let Some(ti) = Atom::from(ident) {
        inner += ti.to_cxx();
    } else {
        inner += &ident.to_string();
    };
    instance += &ident.to_string();
    let instance = instance.replace("::", "$");

    writeln!(out, "#ifndef CXXBRIDGE01_vector_{}", instance);
    writeln!(out, "#define CXXBRIDGE01_vector_{}", instance);
    writeln!(
        out,
        "size_t cxxbridge01$std$vector${}$length(const std::vector<{}> &s) noexcept {{",
        instance, inner,
    );
    writeln!(out, "  return s.size();");
    writeln!(out, "}}");

    writeln!(
        out,
        "void cxxbridge01$std$vector${}$push_back(std::vector<{}> &s, const {} &item) noexcept {{",
        instance, inner, inner
    );
    writeln!(out, "  s.push_back(item);");
    writeln!(out, "}}");

    writeln!(
        out,
        "const {} *cxxbridge01$std$vector${}$get_unchecked(const std::vector<{}> &s, size_t pos) noexcept {{",
        inner, instance, inner,
    );
    writeln!(out, "  return &s[pos];");
    writeln!(out, "}}");
    writeln!(out, "#endif // CXXBRIDGE01_vector_{}", instance);
}
