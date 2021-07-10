use crate::BytesMut;
use crate::{
    config::LanguageConfig,
    error::{Error, Result},
    gen::CodeGen,
    model::{
        codegen_rust_trait, get_operation, get_trait, is_opt_namespace, serialization_trait,
        wasmcloud_model_namespace, CommentKind,
    },
    model_gen::{CodegenRust, Serialization, Wasmbus},
    render::Renderer,
    writer::Writer,
    JsonValue, ParamMap,
};
use atelier_core::model::shapes::ShapeKind;
use atelier_core::{
    model::{
        shapes::{
            AppliedTraits, HasTraits, ListOrSet, Map as MapShape, MemberShape, Operation, Service,
            Simple, StructureOrUnion,
        },
        values::Value,
        HasIdentity, Identifier, Model, NamespaceID, ShapeID,
    },
    prelude::{
        prelude_namespace_id, prelude_shape_named, SHAPE_BIGDECIMAL, SHAPE_BIGINTEGER, SHAPE_BLOB,
        SHAPE_BOOLEAN, SHAPE_BYTE, SHAPE_DOCUMENT, SHAPE_DOUBLE, SHAPE_FLOAT, SHAPE_INTEGER,
        SHAPE_LONG, SHAPE_PRIMITIVEBOOLEAN, SHAPE_PRIMITIVEBYTE, SHAPE_PRIMITIVEDOUBLE,
        SHAPE_PRIMITIVEFLOAT, SHAPE_PRIMITIVEINTEGER, SHAPE_PRIMITIVELONG, SHAPE_PRIMITIVESHORT,
        SHAPE_SHORT, SHAPE_STRING, SHAPE_TIMESTAMP, TRAIT_DEPRECATED, TRAIT_DOCUMENTATION,
        TRAIT_TRAIT, TRAIT_UNSTABLE,
    },
};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    string::ToString,
};

/// Default templates
pub const RUST_TEMPLATES: &[(&str, &str)] = &[
    (
        "rust.actor.manifest.yaml",
        include_str!("../templates/rust/rust.actor.manifest.yaml.hbs"),
    ),
    (
        "rust.actor.rs",
        include_str!("../templates/rust/rust.actor.rs.hbs"),
    ),
    (
        "rust.Cargo.toml",
        include_str!("../templates/rust/rust.Cargo.toml.hbs"),
    ),
    (
        "rust.cargo_config.toml",
        include_str!("../templates/rust/rust.cargo_config.toml.hbs"),
    ),
    (
        "rust.gitignore",
        include_str!("../templates/rust/rust.gitignore.hbs"),
    ),
    (
        "rust.build.rs",
        include_str!("../templates/rust/rust.build.rs.hbs"),
    ),
    ("ping.smithy", include_str!("../templates/ping.smithy.hbs")),
    (
        "rust.lib.rs",
        include_str!("../templates/rust/rust.lib.rs.hbs"),
    ),
    (
        "rust.Makefile",
        include_str!("../templates/rust/rust.Makefile.hbs"),
    ),
];

const DEFAULT_MAP_TYPE: &str = "std::collections::HashMap";
const DEFAULT_LIST_TYPE: &str = "Vec";
const DEFAULT_SET_TYPE: &str = "std::collections::BTreeSet";
const DEFAULT_DOCUMENT_TYPE: &str = "Vec<u8>";

/// declarations for sorting. First sort key is the type (simple, then map, then struct).
/// In rust, sorting by BytesMut as the second key will result in sort by item name.
#[derive(Eq, Ord, PartialOrd, PartialEq)]
struct Declaration(u8, BytesMut);

type ShapeList<'model> = Vec<(&'model ShapeID, &'model AppliedTraits, &'model ShapeKind)>;

// Modifiers on data type
// This enum may be extended in the future if other variations are required.
// It's recursively composable, so you could represent &Option<&Value>
// with `Ty::Ref(Ty::Opt(Ty::Ref(id)))`
enum Ty<'typ> {
    /// write a plain shape declaration
    Shape(&'typ ShapeID),
    /// write a type wrapped in Option<>
    Opt(&'typ ShapeID),
    /// write a reference type: preceeded by &
    Ref(&'typ ShapeID),
}

#[derive(Default)]
pub struct RustCodeGen {
    /// if set, limits declaration output to this namespace only
    namespace: Option<NamespaceID>,
}

impl CodeGen for RustCodeGen {
    /// Initialize code generator and renderer for language output.j
    /// This hook is called before any code is generated and can be used to initialize code generator
    /// and/or perform additional processing before output files are created.
    fn init(
        &mut self,
        _model: Option<&Model>,
        _lc: &LanguageConfig,
        _output_dir: &Path,
        renderer: &mut Renderer,
    ) -> std::result::Result<(), Error> {
        for t in RUST_TEMPLATES.iter() {
            renderer.add_template(*t)?;
        }
        self.namespace = None;
        Ok(())
    }

    /// After code generation has completed for all files, this method is called once per output language
    /// to allow code formatters to run. The `files` parameter contains a list of all files written or updated.
    fn format(&mut self, files: Vec<PathBuf>) -> Result<()> {
        // make a list of all output files with ".rs" extension so we can fix formatting with rustfmt
        // minor nit: we don't check the _config-only flag so there could be some false positives here, but rustfmt is safe to use anyway
        let rust_sources = files
            .into_iter()
            .filter(is_rust_source)
            .collect::<Vec<PathBuf>>();

        if !rust_sources.is_empty() {
            let formatter = crate::rustfmt::RustFmtCommand::default();
            formatter.execute(rust_sources)?;
        }
        Ok(())
    }

    /// Perform any initialization required prior to code generation for a file
    /// `model` may be used to check model metadata
    /// `id` is a tag from codegen.toml that indicates which source file is to be written
    /// `namespace` is the namespace in the model to generate
    #[allow(unused_variables)]
    fn init_file(
        &mut self,
        w: &mut Writer,
        model: &Model,
        file_config: &crate::config::OutputFile,
        params: &ParamMap,
    ) -> Result<()> {
        self.namespace = match &file_config.namespace {
            Some(ns) => Some(NamespaceID::from_str(ns)?),
            None => None,
        };
        Ok(())
    }

    fn write_source_file_header(
        &mut self,
        w: &mut Writer,
        model: &Model,
        params: &ParamMap,
    ) -> Result<()> {
        // special case: if the crate we are generating is "wasmbus_rpc" then we have to import it with "crate::".
        let import_crate = match params.get("crate") {
            Some(JsonValue::String(c)) if c == "wasmbus_rpc" => "crate",
            _ => "wasmbus_rpc",
        };
        w.write(&format!(
            r#"// This file is generated automatically using wasmcloud-weld and smithy model definitions
//
#[allow(unused_imports)]
use {}::{{
    client, context, deserialize, serialize, MessageDispatch, RpcError,
    Transport, Message,
}};
#[allow(unused_imports)] use async_trait::async_trait;
#[allow(unused_imports)] use serde::{{Deserialize, Serialize}};
#[allow(unused_imports)] use std::borrow::Cow;
"#, import_crate))
        ;

        w.write(&format!(
            "\npub const SMITHY_VERSION : &str = \"{}\";\n\n",
            model.smithy_version().to_string()
        ));
        Ok(())
    }

    fn declare_types(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        let ns = self.namespace.clone();

        let mut shapes = model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
            .collect::<ShapeList>();
        // sort shapes (they are all in the same namespace if ns.is_some(), which is usually true)
        shapes.sort_by_key(|v| v.0);

        for (id, traits, shape) in shapes.into_iter() {
            match shape {
                ShapeKind::Simple(simple) => {
                    self.declare_simple_shape(&mut w, id.shape_name(), traits, simple)?;
                }
                ShapeKind::Map(map) => {
                    self.declare_map_shape(&mut w, id.shape_name(), traits, map)?;
                }
                ShapeKind::List(list) => {
                    self.declare_list_or_set_shape(
                        &mut w,
                        id.shape_name(),
                        traits,
                        list,
                        DEFAULT_LIST_TYPE,
                    )?;
                }
                ShapeKind::Set(set) => {
                    self.declare_list_or_set_shape(
                        &mut w,
                        id.shape_name(),
                        traits,
                        set,
                        DEFAULT_SET_TYPE,
                    )?;
                }
                ShapeKind::Structure(strukt) => {
                    //if !traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap()) {
                    self.declare_structure_shape(&mut w, id.shape_name(), traits, strukt)?;
                    //}
                }
                ShapeKind::Operation(_)
                | ShapeKind::Resource(_)
                | ShapeKind::Service(_)
                | ShapeKind::Union(_)
                | ShapeKind::Unresolved => {}
            }
        }
        Ok(())
    }

    fn write_services(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        _params: &ParamMap,
    ) -> Result<()> {
        let ns = self.namespace.clone();
        for (id, traits, shape) in model
            .shapes()
            .filter(|s| is_opt_namespace(s.id(), &ns))
            .map(|s| (s.id(), s.traits(), s.body()))
        {
            if let ShapeKind::Service(service) = shape {
                self.write_service_interface(&mut w, model, id.shape_name(), traits, service)?;
                self.write_service_receiver(&mut w, model, id.shape_name(), traits, service)?;
                self.write_service_sender(&mut w, model, id.shape_name(), traits, service)?;
            }
        }
        Ok(())
    }

    /// Write a single-line comment
    fn write_comment(&mut self, w: &mut Writer, kind: CommentKind, line: &str) {
        w.write(match kind {
            CommentKind::Documentation => "/// ",
            CommentKind::Inner => "// ",
        });
        w.write(line);
        w.write(b"\n");
    }

    /// returns rust source file extension "rs"
    fn get_file_extension(&self) -> &'static str {
        "rs"
    }
}

/// returns true if the file path ends in ".rs"
#[allow(clippy::ptr_arg)]
fn is_rust_source(path: &PathBuf) -> bool {
    match path.extension() {
        Some(s) => s.to_string_lossy().as_ref() == "rs",
        _ => false,
    }
}

impl RustCodeGen {
    /// Apply documentation traits: (documentation, deprecated, unstable)
    fn apply_documentation_traits(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
    ) {
        if let Some(Some(Value::String(text))) =
            traits.get(&prelude_shape_named(TRAIT_DOCUMENTATION).unwrap())
        {
            self.write_documentation(&mut w, id, text);
        }

        // deprecated
        if let Some(Some(Value::Object(map))) =
            traits.get(&prelude_shape_named(TRAIT_DEPRECATED).unwrap())
        {
            w.write(b"#[deprecated(");
            if let Some(Value::String(since)) = map.get("since") {
                w.write(&format!("since=\"{}\"\n", since));
            }
            if let Some(Value::String(message)) = map.get("message") {
                w.write(&format!("note=\"{}\"\n", message));
            }
            w.write(b")\n");
        }

        // unstable
        if traits
            .get(&prelude_shape_named(TRAIT_UNSTABLE).unwrap())
            .is_some()
        {
            self.write_comment(&mut w, CommentKind::Documentation, "@unstable");
        }
    }

    /// Write a type name, a primitive or defined type, with or without deref('&') and with or without Option<>
    fn write_type(&mut self, w: &mut Writer, ty: Ty<'_>) -> Result<()> {
        match ty {
            Ty::Opt(id) => {
                w.write(b"Option<");
                self.write_type(w, Ty::Shape(id))?;
                w.write(b">");
            }
            Ty::Ref(id) => {
                w.write(b"&");
                self.write_type(w, Ty::Shape(id))?;
            }
            Ty::Shape(id) => {
                let name = id.shape_name().to_string();
                if id.namespace() == prelude_namespace_id() {
                    let ty = match name.as_ref() {
                        // Document are  Blob
                        SHAPE_BLOB => "Vec<u8>",
                        SHAPE_BOOLEAN | SHAPE_PRIMITIVEBOOLEAN => "bool",
                        SHAPE_STRING => "String",
                        SHAPE_BYTE | SHAPE_PRIMITIVEBYTE => "i8",
                        SHAPE_SHORT | SHAPE_PRIMITIVESHORT => "i16",
                        SHAPE_INTEGER | SHAPE_PRIMITIVEINTEGER => "i32",
                        SHAPE_LONG | SHAPE_PRIMITIVELONG => "i64",
                        SHAPE_FLOAT | SHAPE_PRIMITIVEFLOAT => "float32",
                        SHAPE_DOUBLE | SHAPE_PRIMITIVEDOUBLE => "float64",
                        // if declared as members (of a struct, list, or map), we don't have trait data here to write
                        // as anything other than a blob. Instead, a type should be created for the Document that can have traits,
                        // and that type used for the member. This should probably be a lint rule.
                        SHAPE_DOCUMENT => DEFAULT_DOCUMENT_TYPE,
                        SHAPE_TIMESTAMP => {
                            cfg_if::cfg_if! {
                                if #[cfg(feature = "Timestamp")] { "Timestamp" } else { return Err(Error::UnsupportedTimestamp) }
                            }
                        }
                        SHAPE_BIGINTEGER => {
                            cfg_if::cfg_if! {
                                if #[cfg(feature = "BigInteger")] { "BigInteger" } else { return Err(Error::UnsupportedBigInteger) }
                            }
                        }
                        SHAPE_BIGDECIMAL => {
                            cfg_if::cfg_if! {
                                if #[cfg(feature = "BigDecimal")] { "BigDecimal" } else { return Err(Error::UnsupportedBigDecimal) }
                            }
                        }
                        _ => return Err(Error::UnsupportedType(name)),
                    };
                    w.write(ty);
                } else if id.namespace() == wasmcloud_model_namespace() {
                    let ty = match name.as_ref() {
                        "U64" => "u64",
                        "U32" => "u32",
                        "U8" => "u8",
                        "I64" => "i64",
                        "I32" => "i32",
                        "I8" => "i8",
                        "U16" => "u16",
                        "I16" => "i16",
                        other => other, // just write the shape name
                    };
                    w.write(ty);
                } else {
                    // TODO: need to be able to lookup from namespace to canonical module path
                    // TODO: need to use explicit lib path
                    w.write(&self.to_type_name(&id.shape_name().to_string()));
                }
            }
        }
        Ok(())
    }

    /// append suffix to type name, for example "Game", "Context" -> "GameContext"
    fn write_ident_with_suffix(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        suffix: &str,
    ) -> Result<()> {
        self.write_ident(&mut w, id);
        w.write(suffix); // assume it's already PascalCalse
        Ok(())
    }

    // declaration for simple type
    fn declare_simple_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        simple: &Simple,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, id, traits);
        w.write(b"pub type ");
        self.write_ident(&mut w, id);
        w.write(b" = ");
        let ty = match simple {
            Simple::Blob => "Vec<u8>",
            Simple::Boolean => "bool",
            Simple::String => "String",
            Simple::Byte => "i8",
            Simple::Short => "i16",
            Simple::Integer => "i32",
            Simple::Long => "i64",
            Simple::Float => "f32",
            Simple::Double => "f64",

            // note: traits may modify this
            Simple::Document => DEFAULT_DOCUMENT_TYPE,

            Simple::Timestamp => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "Timestamp")] { "Timestamp" } else { return Err(Error::UnsupportedTimestamp) }
                }
            }
            Simple::BigInteger => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "BigInteger")] { "BigInteger" } else { return Err(Error::UnsupportedBigInteger) }
                }
            }
            Simple::BigDecimal => {
                cfg_if::cfg_if! {
                    if #[cfg(feature = "BigDecimal")] { "BigDecimal" } else { return Err(Error::UnsupportedBigDecimal) }
                }
            }
        };
        w.write(ty);
        w.write(b";\n\n");
        Ok(())
    }

    fn declare_map_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &MapShape,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, id, traits);
        w.write(b"pub type ");
        self.write_ident(&mut w, id);
        w.write(b" = ");
        w.write(DEFAULT_MAP_TYPE);
        w.write(b"<");
        self.write_type(&mut w, Ty::Shape(shape.key().target()))?;
        w.write(b",");
        self.write_type(&mut w, Ty::Shape(shape.value().target()))?;
        w.write(b">;\n\n");
        Ok(())
    }

    fn declare_list_or_set_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        shape: &ListOrSet,
        typ: &str,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, id, traits);
        w.write(b"pub type ");
        self.write_ident(&mut w, id);
        w.write(b" = ");
        w.write(typ);
        w.write(b"<");
        self.write_type(&mut w, Ty::Shape(shape.member().target()))?;
        w.write(b">;\n\n");
        Ok(())
    }

    fn declare_structure_shape(
        &mut self,
        mut w: &mut Writer,
        id: &Identifier,
        traits: &AppliedTraits,
        strukt: &StructureOrUnion,
    ) -> Result<()> {
        let is_trait_struct = traits.contains_key(&prelude_shape_named(TRAIT_TRAIT).unwrap());
        self.apply_documentation_traits(&mut w, id, traits);
        let derive_default =
            if let Some(cg) = get_trait::<CodegenRust>(traits, codegen_rust_trait())? {
                cg.derive_default
            } else {
                false
            };
        //if let Some(Some(Value::Object(cg_map))) = traits.get(codegen_rust_trait()) {
        //    if let Some(Value::Boolean(enable)) = cg_map.get("deriveDefault") {
        //        derive_default = *enable;
        //    }
        //};
        w.write(b"#[derive(");
        if derive_default {
            w.write(b"Default, ")
        }
        w.write(b"Clone, Debug, Serialize, Deserialize)]\n");
        w.write(b"pub struct ");
        self.write_ident(&mut w, id);
        w.write(b" {\n");
        for member in strukt.members() {
            self.apply_documentation_traits(&mut w, member.id(), member.traits());

            // use the declared name for serialization, unless an override is declared
            // with `@sesrialization(name: SNAME)`
            let ser_name = if let Some(Serialization {
                name: Some(ser_name),
            }) = get_trait(member.traits(), serialization_trait())?
            {
                ser_name
            } else {
                member.id().to_string()
            };
            //let declared_name = member.id().to_string();
            let rust_field_name = self.to_field_name(member.id())?;
            if ser_name != rust_field_name {
                w.write(&format!("  #[serde(rename=\"{}\")] ", ser_name));
            }
            // for rmp-msgpack - need to use serde_bytes serializer for Blob (and Option<Blob>)
            if member.target() == &ShapeID::new_unchecked("smithy.api", "Blob", None) {
                w.write(r#"  #[serde(with="serde_bytes")] "#);
            }
            if is_optional_type(member) {
                w.write(r#"  #[serde(default, skip_serializing_if = "Option::is_none")]"#);
            } else if is_trait_struct && !member.is_required() {
                // trait structs are deserialized only and need default values if not required
                w.write(r#"  #[serde(default)]"#);
            }
            w.write(b"  pub ");
            w.write(&rust_field_name);
            w.write(b": ");
            self.write_field_type(&mut w, member)?;
            w.write(b",\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// write field type, wrapping with Option if field is not required
    fn write_field_type(&mut self, mut w: &mut Writer, field: &MemberShape) -> Result<()> {
        self.write_type(
            &mut w,
            if is_optional_type(field) {
                Ty::Opt(field.target())
            } else {
                Ty::Shape(field.target())
            },
        )
    }

    /// Declares the service as a rust Trait whose methods are the smithy service operations
    fn write_service_interface(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        self.apply_documentation_traits(&mut w, service_id, service_traits);
        let wasmbus: Option<Wasmbus> = get_trait(service_traits, crate::model::wasmbus_trait())?;
        if let Some(wasmbus) = wasmbus {
            if let Some(contract) = wasmbus.contract_id {
                let text = format!("wasmbus.contractId: {}", &contract);
                self.write_documentation(&mut w, service_id, &text);
            }
            if wasmbus.provider_receive {
                let text = "wasmbus.providerReceive";
                self.write_documentation(&mut w, service_id, text);
            }
            if wasmbus.actor_receive {
                let text = "wasmbus.actorReceive";
                self.write_documentation(&mut w, service_id, text);
            }
        }

        w.write(b"#[async_trait]\npub trait ");
        self.write_ident(&mut w, service_id);
        w.write(b"{\n");
        //HERE
        for operation in service.operations() {
            // if operation is not declared in this namespace, don't define it here
            if let Some(ref ns) = self.namespace {
                if operation.namespace() != ns {
                    continue;
                }
            }
            let (op, op_traits) = get_operation(model, operation, service_id)?;

            // TODO: re-think what to do if operation is in another namspace and self.namespace is None
            let method_id = operation.shape_name();
            self.write_method_signature(&mut w, method_id, op_traits, op)?;
            w.write(b";\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }

    /// write trait function declaration "async fn method(args) -> Result< return_type, RpcError >"
    /// does not write trailing semicolon so this can be used for declaration and implementation
    fn write_method_signature(
        &mut self,
        mut w: &mut Writer,
        method_id: &Identifier,
        method_traits: &AppliedTraits,
        op: &Operation,
    ) -> Result<()> {
        let method_name = self.to_method_name(method_id);
        self.apply_documentation_traits(&mut w, method_id, method_traits);
        w.write(b"async fn ");
        w.write(&method_name);
        w.write(b"(&self, ctx: &context::Context<'_>");
        if let Some(input_type) = op.input() {
            w.write(b", arg: "); // pass arg by reference
            self.write_type(&mut w, Ty::Ref(input_type))?;
        }
        w.write(b") -> Result<");
        if let Some(output_type) = op.output() {
            self.write_type(&mut w, Ty::Shape(output_type))?;
        } else {
            w.write(b"()");
        }
        w.write(b", RpcError>");
        Ok(())
    }

    // pub trait FooReceiver : MessageDispatch + Foo { ... }
    fn write_service_receiver(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Receiver receives messages defined in the {} service trait",
            service_id, service_id
        );
        self.write_comment(&mut w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(&mut w, service_id, service_traits);
        w.write(b"#[async_trait]\npub trait ");
        self.write_ident_with_suffix(&mut w, service_id, "Receiver")?;
        w.write(b" : MessageDispatch + ");
        self.write_ident(&mut w, service_id);
        w.write(
            br#"{
            async fn dispatch(
                &self,
                ctx: &context::Context<'_>,
                message: &Message<'_> ) -> Result< Message<'static>, RpcError> {
                match message.method {
        "#,
        );

        for method_id in service.operations() {
            // TODO: if it's valid for a service to include operations from another namespace, then this isn't doing the right thing
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();
            let (op, _) = get_operation(model, method_id, service_id)?;
            w.write(b"\"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\" => {\n");
            if op.has_input() {
                // let value : InputType = deserialize(...)?;
                w.write(b"let value: ");
                // TODO: should this be input.target?
                self.write_type(&mut w, Ty::Shape(op.input().as_ref().unwrap()))?;
                w.write(b" = deserialize(message.arg.as_ref())?;\n");
            }
            // let resp = Trait::method(self, ctx, &value).await?;
            w.write(b"let resp = ");
            self.write_ident(&mut w, service_id); // Service::method
            w.write(b"::");
            w.write(&self.to_method_name(method_ident));
            w.write(b"(self, ctx");
            if op.has_input() {
                w.write(b", &value");
            }
            w.write(b").await?;\n");

            // serialize result
            w.write(b"let buf = Cow::Owned(serialize(&resp)?);\n");
            //w.write(br#"console_log(format!("actor result {}b",buf.len())); "#);
            w.write(b"Ok(Message { method: ");
            w.write(&self.full_dispatch_name(service_id, method_ident));
            w.write(b", arg: buf })},\n");
        }
        w.write(b"_ => Err(RpcError::MethodNotHandled(format!(\"");
        self.write_ident(&mut w, service_id);
        w.write(b"::{}\", message.method))),\n");
        w.write(b"}\n}\n}\n\n"); // end match, end fn dispatch, end trait

        Ok(())
    }

    /// writes the service sender struct and constructor
    // pub struct FooSender{ ... }
    fn write_service_sender(
        &mut self,
        mut w: &mut Writer,
        model: &Model,
        service_id: &Identifier,
        service_traits: &AppliedTraits,
        service: &Service,
    ) -> Result<()> {
        let doc = format!(
            "{}Sender sends messages to a {} service",
            service_id, service_id
        );
        self.write_comment(&mut w, CommentKind::Documentation, &doc);
        self.apply_documentation_traits(&mut w, service_id, service_traits);
        w.write(b"#[derive(Debug)]\npub struct ");
        self.write_ident_with_suffix(&mut w, service_id, "Sender")?;
        w.write(b"<T> { transport: T, config: client::SendConfig }\n\n");

        // implement constructor for TraitClient
        w.write(b"impl<T:Transport>  ");
        self.write_ident_with_suffix(&mut w, service_id, "Sender")?;
        w.write(b"<T> { \n");
        w.write(b" pub fn new(config: client::SendConfig, transport: T) -> Self { ");
        self.write_ident_with_suffix(&mut w, service_id, "Sender")?;
        w.write(b"{ transport, config }\n}\n}\n\n");

        // implement Trait for TraitSender
        w.write(b"#[async_trait]\nimpl<T:Transport + std::marker::Sync + std::marker::Send> ");
        self.write_ident(&mut w, service_id);
        w.write(b" for ");
        self.write_ident_with_suffix(&mut w, service_id, "Sender")?;
        w.write(b"<T> {\n");

        for method_id in service.operations() {
            // TODO: if it's valid for a service to include operations from another namespace, then this isn't doing the right thing
            // we don't add operations defined in another namespace
            if let Some(ref ns) = self.namespace {
                if method_id.namespace() != ns {
                    continue;
                }
            }
            let method_ident = method_id.shape_name();

            let (op, method_traits) = get_operation(model, method_id, service_id)?;
            w.write(b"#[allow(unused)]\n");
            self.write_method_signature(&mut w, method_ident, method_traits, op)?;
            w.write(b" {\n");

            if op.has_input() {
                w.write(b"let arg = serialize(arg)?;\n");
            } else {
                w.write(b"let arg = *b\"\";\n");
            }
            w.write(b"let resp = self.transport.send(ctx, &self.config, Message{ method: ");
            // TODO: switch to quoted full method (increment api version # if not legacy)
            //w.write(self.full_dispatch_name(trait_base.id(), method_id));
            // note: legacy is just the latter part
            w.write(b"\"");
            w.write(&self.op_dispatch_name(method_ident));
            w.write(b"\", arg: Cow::Borrowed(&arg)}).await?;\n");
            if op.has_output() {
                w.write(b"let value = deserialize(resp.arg.as_ref())?; Ok(value)");
            } else {
                w.write(b"Ok(())");
            }
            w.write(b" }\n");
        }
        w.write(b"}\n\n");
        Ok(())
    }
} // impl CodeGenRust

/// is_optional_type determines whether the field should be wrapped in Option<>
/// the value is true if it has an explicit `box` trait, or if it's
/// un-annotated and not one of (boolean, byte, short, integer, long, float, double)
fn is_optional_type(field: &MemberShape) -> bool {
    field.is_boxed()
        || (!field.is_required()
            && ![
                "Boolean", "Byte", "Short", "Integer", "Long", "Float", "Double",
            ]
            .contains(&field.target().shape_name().to_string().as_str()))
}

/*
Opt   @required   @box    bool/int/...
1     0           0       0
0     0           0       1
1     0           1       0
1     0           1       1
0     1           0       0
0     1           0       1
x     1           1       0
x     1           1       1
*/
