use crate::symbols::{
    AbiInfo, ConstInfo, FuncInfo, SymbolId, SymbolInformation, Symbols, TypeInfo, VarInfo,
};
use crate::{
    ast::{
        Abi, AbiElem, Block, BlockExpr, EffectDecl, Expr, ExprOrStatement, FieldAccessExpression,
        FnDef, FnType, Identifier, LoopBody, PrimaryExpr, ProgramItem, Script, Sig, Spanned,
        StarstreamProgram, Statement, Token, TokenItem, TypeArg, TypeDef, TypeDefRhs, TypeOrSelf,
        TypeRef, TypedBindings, Utxo, UtxoItem,
    },
    typechecking::EffectSet,
};
use ariadne::{Color, Label, Report, ReportKind};
use chumsky::span::SimpleSpan;
use std::collections::{HashMap, HashSet};

/// This traverses the AST, and assigns an unique numeric ID to each identifier
/// on declaration. The ids are stored inside of the Identifier node of the AST.
///
/// Also references are resolved when found, according to the scoping rules.
/// These can then be used to index into the Symbols table to get information about
/// the declaration of that particular identifier.
///
/// This pass does _not_ do resolution of field accesses or method calls, since that
/// usually requires information about the types. Although it may be possible to
/// resolve functions in builtin types.
pub fn do_scope_analysis(
    mut program: StarstreamProgram,
) -> Result<(StarstreamProgram, Symbols), Vec<Report<'static>>> {
    let mut resolver = Visitor::new();
    resolver.visit_program(&mut program);
    let (symbols, errors) = resolver.finish();

    if !errors.is_empty() {
        Err(errors)
    } else {
        Ok((program, symbols))
    }
}

#[derive(Debug, Default)]
pub struct Scope {
    var_declarations: HashMap<String, SymbolId>,
    function_declarations: HashMap<String, SymbolId>,
    type_declarations: HashMap<String, SymbolId>,
    abi_declarations: HashMap<String, SymbolId>,
    is_function_scope: Option<SymbolId>,
    is_type_scope: Option<SymbolId>,
}

pub const STARSTREAM_ENV: &str = "StarstreamEnv";
pub const STARSTREAM: &str = "Starstream";

struct Visitor {
    stack: Vec<Scope>,
    // used to keep count of variables declared in the innermost function scope it's
    // kept outside the scope stack to avoid having to do parent traversal,
    // since not all scopes are function scopes.
    locals: Vec<Vec<SymbolId>>,
    // used to generate unique ids for new identifiers
    symbol_counter: u64,
    errors: Vec<Report<'static>>,
    symbols: Symbols,
}

#[derive(Debug, Clone)]
pub enum SymbolKind {
    Variable,
    Function,
    Type,
    Abi,
    Namespace,
}

impl Visitor {
    fn new() -> Self {
        Visitor {
            stack: vec![],
            locals: vec![],
            symbol_counter: 0,
            errors: vec![],
            symbols: Symbols::default(),
        }
    }

    fn push_type_scope(&mut self, type_id: SymbolId) {
        self.stack.push(Scope {
            is_type_scope: Some(type_id),
            ..Default::default()
        });
    }

    fn push_function_scope(&mut self, f: SymbolId) {
        self.stack.push(Scope {
            is_function_scope: Some(f),
            ..Default::default()
        });

        self.locals.push(vec![]);
    }

    fn push_scope(&mut self) {
        self.stack.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        let scope = self.stack.pop();

        if let Some(scope) = scope {
            if let Some(function) = scope.is_function_scope {
                let locals = self.locals.pop().unwrap();

                self.symbols
                    .functions
                    .get_mut(&function)
                    .unwrap()
                    .info
                    .locals = locals;
            }
        }
    }

    fn finish(self) -> (Symbols, Vec<Report<'static>>) {
        (self.symbols, self.errors)
    }

    // TODO: mostly just to get the examples working
    // these probably would have to be some sort of import?
    fn add_builtins(&mut self) {
        self.push_type_declaration(&mut Identifier::new("Option", None), None);
        self.push_type_declaration(&mut Identifier::new("any", None), None);
        self.push_type_declaration(&mut Identifier::new("Value", None), None);

        self.push_function_declaration(
            &mut Identifier::new("assert", None),
            FuncInfo {
                inputs_ty: vec![TypeArg::Bool],
                output_ty: None,
                effects: EffectSet::empty(),
                locals: vec![],
                ..Default::default()
            },
        );

        self.push_function_declaration(
            &mut Identifier::new("None", None),
            FuncInfo {
                inputs_ty: vec![],
                output_ty: None,
                effects: EffectSet::empty(),
                locals: vec![],
                ..Default::default()
            },
        );

        self.push_function_declaration(
            &mut Identifier::new("print", None),
            FuncInfo {
                inputs_ty: vec![TypeArg::String],
                output_ty: None,
                effects: EffectSet::empty(),
                locals: vec![],
                ..Default::default()
            },
        );

        self.push_function_declaration(
            &mut Identifier::new("IsTxSignedBy", None),
            FuncInfo {
                inputs_ty: vec![TypeArg::U32],
                output_ty: Some(TypeArg::Bool),
                effects: EffectSet::empty(),
                locals: vec![],
                ..Default::default()
            },
        );

        self.push_constant_declaration(&mut Identifier::new("context", None));

        let any = Box::new(TypeArg::TypeRef(TypeRef(Identifier::new("any", None))));

        self.visit_type_def(&mut TypeDef {
            name: Identifier::new("PublicKey", None),
            ty: TypeDefRhs::TypeArg(TypeArg::U32),
        });

        let mut abi = Abi {
            name: Identifier::new("Starstream", None),
            values: vec![AbiElem::EffectDecl(EffectDecl::EffectSig(Sig {
                name: Identifier::new("TokenUnbound", None),
                input_types: vec![
                    TypeArg::Intermediate {
                        abi: any.clone(),
                        storage: any.clone(),
                    },
                    TypeArg::U32,
                ],
                output_type: None,
            }))],
        };

        self.visit_abi(&mut abi);
        self.symbols
            .builtins
            .insert(STARSTREAM, abi.name.uid.unwrap());

        let mut abi = Abi {
            name: Identifier::new("StarstreamEnv", None),
            values: vec![
                AbiElem::EffectDecl(EffectDecl::EffectSig(Sig {
                    name: Identifier::new("Caller", None),
                    input_types: vec![],
                    output_type: Some(TypeArg::U32),
                })),
                AbiElem::EffectDecl(EffectDecl::EffectSig(Sig {
                    name: Identifier::new("ThisCode", None),
                    input_types: vec![],
                    output_type: Some(TypeArg::U32),
                })),
                AbiElem::EffectDecl(EffectDecl::EffectSig(Sig {
                    name: Identifier::new("CoordinationCode", None),
                    input_types: vec![],
                    output_type: Some(TypeArg::U32),
                })),
                AbiElem::EffectDecl(EffectDecl::EffectSig(Sig {
                    name: Identifier::new("Caller", None),
                    input_types: vec![],
                    output_type: Some(TypeArg::U32),
                })),
                AbiElem::EffectDecl(EffectDecl::EffectSig(Sig {
                    name: Identifier::new("IsTxSignedBy", None),
                    input_types: vec![TypeArg::U32],
                    output_type: Some(TypeArg::Bool),
                })),
            ],
        };
        self.visit_abi(&mut abi);
        self.symbols
            .builtins
            .insert(STARSTREAM_ENV, abi.name.uid.unwrap());

        let namespaces = {
            vec![
                // None in the 3rd element makes it a constructor
                ("PayToPublicKeyHash", "new", None),
                ("List", "new", None),
            ]
        };

        for (builtin, f, ty) in namespaces {
            let mut identifier = Identifier::new(builtin, None);
            let type_id = self.push_type_declaration(&mut identifier, None);

            self.push_type_scope(type_id);

            self.push_function_declaration(
                &mut Identifier::new(f, None),
                ty.unwrap_or(FuncInfo {
                    inputs_ty: vec![],
                    output_ty: Some(TypeArg::TypeRef(TypeRef(identifier.clone()))),
                    effects: EffectSet::empty(),
                    locals: vec![],
                    ..Default::default()
                }),
            );

            self.pop_scope();
        }

        let mut identifier = Identifier::new("Intermediate", None);
        let type_id = self.push_type_declaration(&mut identifier, None);
        self.symbols.builtins.insert("Intermediate", type_id);

        let self_ty = TypeArg::Intermediate {
            abi: any.clone(),
            storage: any,
        };

        let pair = Identifier::new("IntermediatePair", None);
        let mut type_def = TypeDef {
            name: pair,
            ty: TypeDefRhs::Object(TypedBindings {
                values: vec![
                    (Identifier::new("fst", None), self_ty.clone()),
                    (Identifier::new("snd", None), self_ty.clone()),
                ],
            }),
        };
        self.visit_type_def(&mut type_def);

        self.push_type_scope(type_id);

        self.push_function_declaration(
            &mut Identifier::new("change_for", None),
            FuncInfo {
                inputs_ty: vec![TypeArg::U32],
                output_ty: Some(TypeArg::TypeRef(TypeRef(type_def.name.clone()))),
                effects: EffectSet::empty(),
                locals: vec![],
                ..Default::default()
            },
        );

        self.pop_scope();
    }

    fn visit_program(&mut self, program: &mut StarstreamProgram) {
        self.push_scope();

        self.add_builtins();

        for item in &mut program.items {
            match item {
                ProgramItem::TypeDef(type_def) => self.visit_type_def(type_def),
                ProgramItem::Token(token) => {
                    self.push_type_declaration(&mut token.name, None);
                }
                ProgramItem::Script(_script) => (),
                ProgramItem::Utxo(utxo) => {
                    self.push_type_declaration(&mut utxo.name, None);
                }
                ProgramItem::Constant { name, value: _ } => {
                    self.push_constant_declaration(name);
                }
                ProgramItem::Abi(abi) => {
                    self.visit_abi(abi);
                }
            }
        }

        let mut items = program.items.iter_mut().collect::<Vec<_>>();

        items.sort_by_key(|item| match item {
            ProgramItem::Abi(_abi) => 0,
            ProgramItem::Token(_token) => 1,
            ProgramItem::Utxo(_utxo) => 2,
            ProgramItem::TypeDef(_type_def) => 3,
            ProgramItem::Constant { name: _, value: _ } => 4,
            ProgramItem::Script(_script) => 5,
        });

        for item in items {
            match item {
                ProgramItem::Script(script) => {
                    self.visit_script(script);
                }
                ProgramItem::Utxo(utxo) => {
                    self.visit_utxo(utxo);
                }
                ProgramItem::Token(token) => {
                    self.visit_token(token);
                }
                _ => (),
            }
        }

        self.pop_scope();
    }

    pub fn visit_script(&mut self, script: &mut Script) {
        self.visit_fn_defs(&mut script.definitions, None, None);
    }

    pub fn visit_utxo(&mut self, utxo: &mut Utxo) {
        let uid = self.push_type_declaration(&mut utxo.name, None);

        // we need to put these into scope before doing anything else
        self.push_type_scope(uid);

        let self_ty = TypeArg::TypeRef(TypeRef(utxo.name.clone()));
        let self_ty_ref = TypeArg::Ref(Box::new(self_ty.clone()));

        let mut effects = self.implicit_effects();
        effects.add(self.symbols.builtins[STARSTREAM]);
        self.push_function_declaration(
            &mut Identifier::new("resume", None),
            FuncInfo {
                inputs_ty: std::iter::once(self_ty.clone())
                    .chain(
                        utxo.items
                            .iter()
                            .filter_map(|item| match item {
                                UtxoItem::Resume(type_arg) => Some(type_arg.clone()),
                                _ => None,
                            })
                            .chain(std::iter::once(TypeArg::Unit))
                            .take(1)
                            .map(|ty| TypeArg::Ref(Box::new(ty))),
                    )
                    .collect(),
                output_ty: Some(self_ty.clone()),
                effects,
                locals: vec![],
                mangled_name: Some(format!("starstream_resume_{}", utxo.name.raw)),
                ..Default::default()
            },
        );

        self.push_function_declaration(
            &mut Identifier::new("attach", None),
            FuncInfo {
                inputs_ty: vec![
                    self_ty.clone(),
                    TypeArg::Intermediate {
                        abi: Box::new(TypeArg::TypeRef(TypeRef(Identifier::new("any", None)))),
                        storage: Box::new(TypeArg::TypeRef(TypeRef(Identifier::new("any", None)))),
                    },
                ],
                output_ty: Some(self_ty.clone()),
                effects: EffectSet::empty(),
                locals: vec![],
                ..Default::default()
            },
        );

        for item in &mut utxo.items {
            match item {
                UtxoItem::Main(main) => {
                    if let Some(tys) = &mut main.type_sig {
                        for (_ident, ty) in &mut tys.values {
                            self.visit_type_arg(ty);
                        }
                    }

                    // TODO: what should this be actually?
                    // the effects of main?
                    // or the effects before the first yield?
                    let effects = self.implicit_effects();

                    // TODO: may actually want to get the "main" span

                    self.push_function_declaration(
                        &mut main.ident,
                        FuncInfo {
                            // TODO: check that this matches the storage declaration
                            inputs_ty: main
                                .type_sig
                                .as_ref()
                                .map(|args| args.values.iter().map(|arg| arg.1.clone()).collect())
                                .unwrap_or(vec![]),
                            output_ty: Some(self_ty_ref.clone()),
                            effects,
                            locals: vec![],
                            mangled_name: Some(format!("starstream_new_{}_new", utxo.name.raw)),
                            ..Default::default()
                        },
                    );

                    self.push_function_scope(main.ident.uid.unwrap());

                    if let Some(tys) = &mut main.type_sig {
                        for (ident, _ty) in &mut tys.values {
                            self.push_var_declaration(ident, false);
                        }
                    }

                    let mut implicit_self_var = Identifier::new("self", None);
                    self.push_var_declaration(&mut implicit_self_var, false);
                    let self_ty_normalized = self_ty_ref.canonical_form(&self.symbols);
                    self.symbols
                        .vars
                        .get_mut(&implicit_self_var.uid.unwrap())
                        .unwrap()
                        .info
                        .ty
                        .replace(self_ty_normalized);

                    self.visit_block(&mut main.block, true);
                    self.pop_scope();
                }
                UtxoItem::Impl(utxo_impl) => {
                    let Some((abi, _)) = self.resolve_name(&mut utxo_impl.name, SymbolKind::Abi)
                    else {
                        return;
                    };

                    self.visit_fn_defs(
                        &mut utxo_impl.definitions,
                        Some(self_ty_ref.clone()),
                        Some(abi)
                            .filter(|_| !self.symbols.interfaces[&abi].info.effects.is_empty()),
                    );

                    for definition in &mut utxo_impl.definitions {
                        let Some(abi_def) = self
                            .symbols
                            .interfaces
                            .get(&abi)
                            .unwrap()
                            .info
                            .fns
                            .get(&definition.ident.raw)
                        else {
                            self.push_not_found_error(definition.ident.span.unwrap());
                            return;
                        };

                        let impl_def = self
                            .symbols
                            .functions
                            .get(&definition.ident.uid.unwrap())
                            .unwrap()
                            .info
                            .clone();

                        if !impl_def
                            .inputs_ty
                            .iter()
                            // skip self, assume it's implied
                            .skip(1)
                            .chain(impl_def.output_ty.iter())
                            .zip(abi_def.input_types.iter().chain(abi_def.output_type.iter()))
                            .all(|(impl_def, abi_def)| match (impl_def, abi_def) {
                                // TODO: may want to actually point to the faulty arg in the error
                                (TypeArg::TypeRef(id1), TypeArg::TypeRef(id2)) => {
                                    id1.0.uid.unwrap() == id2.0.uid.unwrap()
                                }
                                (t1, t2) => t1 == t2,
                            })
                        {
                            self.push_abi_mismatch_error(
                                definition.ident.span.unwrap(),
                                abi_def.name.span.unwrap(),
                            );
                        }
                    }

                    self.symbols
                        .types
                        .get_mut(&uid)
                        .unwrap()
                        .info
                        .interfaces
                        .add(abi);
                }
                UtxoItem::Storage(storage) => {
                    let mut storage = storage.clone();

                    for (_identifier, ty) in &mut storage.bindings.values {
                        // TODO: we may need to add the identifier to the
                        // symbols table for codegen
                        self.visit_type_arg(ty);
                    }

                    self.symbols
                        .types
                        .get_mut(&uid)
                        .unwrap()
                        .info
                        .storage
                        .replace(storage);
                }
                // TODO: check that there is only one yield and only one resume
                UtxoItem::Yield(ty) => {
                    self.symbols
                        .types
                        .get_mut(&uid)
                        .unwrap()
                        .info
                        .yield_ty
                        .replace(ty.clone());
                }
                UtxoItem::Resume(ty) => {
                    self.symbols
                        .types
                        .get_mut(&uid)
                        .unwrap()
                        .info
                        .resume_ty
                        .replace(ty.clone());
                }
            }
        }

        self.pop_scope();
    }

    fn implicit_effects(&mut self) -> EffectSet {
        EffectSet::singleton(self.symbols.builtins[STARSTREAM_ENV])
    }

    pub fn visit_token(&mut self, token: &mut Token) {
        let uid = self.push_type_declaration(&mut token.name, None);

        self.push_type_scope(uid);

        let effects = self.implicit_effects();
        self.push_function_declaration(
            &mut Identifier::new("type", None),
            FuncInfo {
                inputs_ty: vec![],
                // TODO: something else
                output_ty: Some(TypeArg::U32),
                effects,
                locals: vec![],
                ..Default::default()
            },
        );

        for item in &mut token.items {
            let effects = self.implicit_effects();

            match item {
                TokenItem::Bind(bind) => {
                    let mut ident =
                        Identifier::new(format!("starstream_bind_{}", token.name.raw), None);

                    self.push_function_declaration(
                        &mut ident,
                        FuncInfo {
                            // TODO: intermediate
                            inputs_ty: vec![],
                            // TODO: handle
                            output_ty: Some(TypeArg::U64),
                            effects,
                            locals: vec![],
                            ..Default::default()
                        },
                    );

                    self.push_function_scope(ident.uid.unwrap());

                    self.push_var_declaration(&mut Identifier::new("self", None), true);
                    self.visit_block(&mut bind.0, false);
                    self.pop_scope();
                }
                TokenItem::Unbind(unbind) => {
                    let mut ident =
                        Identifier::new(format!("starstream_unbind_{}", token.name.raw), None);

                    self.push_function_declaration(
                        &mut ident,
                        FuncInfo {
                            // TODO: handle
                            inputs_ty: vec![TypeArg::U64],
                            // TODO: intermediate
                            output_ty: None,
                            effects,
                            locals: vec![],
                            ..Default::default()
                        },
                    );

                    self.push_function_scope(ident.uid.unwrap());

                    self.push_var_declaration(&mut Identifier::new("self", None), true);
                    self.visit_block(&mut unbind.0, false);
                    self.pop_scope();
                }
                TokenItem::Mint(mint) => {
                    let mut ident = Identifier::new("mint", None);

                    self.push_function_declaration(
                        &mut ident,
                        FuncInfo {
                            inputs_ty: vec![],
                            output_ty: Some(TypeArg::TypeRef(TypeRef(token.name.clone()))),
                            effects,
                            locals: vec![],
                            mangled_name: Some(format!(
                                "starstream_mutate_{}_mint",
                                token.name.raw
                            )),
                            ..Default::default()
                        },
                    );
                    self.push_function_scope(ident.uid.unwrap());
                    self.push_var_declaration(&mut Identifier::new("self", None), true);
                    self.visit_block(&mut mint.0, false);
                    self.pop_scope();
                }
            }
        }

        self.pop_scope();
    }

    pub fn visit_type_def(&mut self, type_def: &mut TypeDef) {
        self.push_type_declaration(&mut type_def.name, Some(type_def.ty.clone()));

        match &mut type_def.ty {
            TypeDefRhs::TypeArg(type_arg) => self.visit_type_arg(type_arg),
            TypeDefRhs::Object(typed_bindings) => {
                for (_name, ty) in &mut typed_bindings.values {
                    // NOTE: we can't resolve field accesses without resolving
                    // the type first.
                    self.visit_type_arg(ty);
                }
            }
            TypeDefRhs::Variant(variant) => {
                for (variant, args) in &mut variant.0 {
                    self.push_function_declaration(
                        variant,
                        FuncInfo {
                            inputs_ty: args.values.iter().map(|arg| arg.1.clone()).collect(),
                            output_ty: Some(TypeArg::TypeRef(TypeRef(type_def.name.clone()))),
                            effects: EffectSet::empty(),
                            locals: vec![],
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }

    fn visit_fn_defs(
        &mut self,
        definitions: &mut [FnDef],
        self_ty: Option<TypeArg>,
        abi: Option<SymbolId>,
    ) {
        for definition in definitions.iter_mut() {
            for arg in &mut definition.inputs {
                match &mut arg.ty {
                    TypeOrSelf::Type(type_arg) => self.visit_type_arg(type_arg),
                    TypeOrSelf::_Self => (),
                }
            }

            if let Some(output_ty) = &mut definition.output {
                self.visit_type_arg(output_ty);
            }

            let mut effects = EffectSet::empty();
            for effect in &mut definition.effects {
                if let Some((symbol_id, _)) = self.resolve_name(effect, SymbolKind::Abi) {
                    effects.add(symbol_id);
                }
            }

            if let Some(abi) = abi {
                effects.add(abi);
            }

            self.push_function_declaration(
                &mut definition.ident,
                FuncInfo {
                    inputs_ty: definition
                        .inputs
                        .iter()
                        .map(|arg| match arg.ty.clone() {
                            TypeOrSelf::Type(type_arg) => type_arg,
                            TypeOrSelf::_Self => self_ty.clone().unwrap(),
                        })
                        .collect(),
                    output_ty: definition.output.clone(),
                    effects,
                    locals: vec![],
                    ..Default::default()
                },
            );
        }

        for definition in definitions {
            self.resolve_name(&mut definition.ident, SymbolKind::Function);

            self.push_function_scope(definition.ident.uid.unwrap());

            for node in &mut definition.inputs {
                self.push_var_declaration(&mut node.name, false);
            }

            self.visit_block(&mut definition.body, false);

            self.pop_scope();
        }
    }

    fn new_symbol(&mut self, ident: &mut Identifier) -> SymbolId {
        let id = self.symbol_counter;
        self.symbol_counter += 1;

        let symbol = SymbolId { id };
        ident.uid.replace(symbol);
        symbol
    }

    fn push_var_declaration(&mut self, ident: &mut Identifier, mutable: bool) -> SymbolId {
        let symbol = self.new_symbol(ident);

        let scope = self.stack.last_mut().unwrap();
        scope.var_declarations.insert(ident.raw.clone(), symbol);

        // TODO: handle error
        let fn_scope = self.locals.last_mut().unwrap();
        let index = fn_scope.len() as u64;
        fn_scope.push(ident.uid.unwrap());
        let var_info = VarInfo {
            index,
            mutable,
            ty: None,
        };

        self.symbols.vars.insert(
            symbol,
            SymbolInformation {
                source: ident.raw.clone(),
                span: ident.span,
                info: var_info,
            },
        );

        symbol
    }

    fn push_constant_declaration(&mut self, ident: &mut Identifier) -> SymbolId {
        let symbol = self.new_symbol(ident);

        let scope = self.stack.last_mut().unwrap();
        scope.var_declarations.insert(ident.raw.clone(), symbol);

        self.symbols.constants.insert(
            symbol,
            SymbolInformation {
                source: ident.raw.clone(),
                span: ident.span,
                info: ConstInfo { ty: None },
            },
        );

        symbol
    }

    fn push_function_declaration(&mut self, ident: &mut Identifier, info: FuncInfo) -> SymbolId {
        let symbol = self.new_symbol(ident);

        self.symbols.functions.insert(
            symbol,
            SymbolInformation {
                source: ident.raw.clone(),
                span: ident.span,
                info,
            },
        );

        let scope = self.stack.last_mut().unwrap();

        if let Some(prev) = scope
            .function_declarations
            .insert(ident.raw.clone(), symbol)
        {
            let prev = self.symbols.functions.get(&prev).unwrap().span.unwrap();

            self.push_redeclaration_error(ident.span.unwrap(), prev);
        }

        let type_scope = self
            .stack
            .iter()
            .rev()
            .find_map(|scope| scope.is_type_scope);

        if let Some(type_scope) = type_scope {
            let type_information = self.symbols.types.get_mut(&type_scope).unwrap();

            let inserted = type_information.info.declarations.insert(symbol);

            if !inserted {
                // fine to unwrap since otherwise inserted would be true
                let prev = type_information.info.declarations.get(&symbol).unwrap();

                // TODO: cleanup the panics (compiler error)
                let prev = self.symbols.functions.get(prev).unwrap();

                self.push_redeclaration_error(ident.span.unwrap(), prev.span.unwrap());
            }
        }

        symbol
    }

    fn push_type_declaration(
        &mut self,
        ident: &mut Identifier,
        type_def: Option<TypeDefRhs>,
    ) -> SymbolId {
        let symbol = self.new_symbol(ident);

        let scope = self.stack.last_mut().unwrap();
        scope.type_declarations.insert(ident.raw.clone(), symbol);

        self.symbols.types.insert(
            symbol,
            SymbolInformation {
                source: ident.raw.clone(),
                span: ident.span,
                info: TypeInfo {
                    declarations: HashSet::new(),
                    type_def,
                    storage: None,
                    yield_ty: None,
                    resume_ty: None,
                    interfaces: EffectSet::empty(),
                },
            },
        );

        symbol
    }

    fn push_interface_declaration(&mut self, ident: &mut Identifier, info: AbiInfo) -> SymbolId {
        let symbol = self.new_symbol(ident);

        let scope = self.stack.last_mut().unwrap();
        scope.abi_declarations.insert(ident.raw.clone(), symbol);

        self.symbols.interfaces.insert(
            symbol,
            SymbolInformation {
                source: ident.raw.clone(),
                span: ident.span,
                info,
            },
        );

        symbol
    }

    fn resolve_name(
        &mut self,
        identifier: &mut Identifier,
        symbol_kind: SymbolKind,
    ) -> Option<(SymbolId, SymbolKind)> {
        let resolution = self.stack.iter().rev().find_map(|scope| match symbol_kind {
            SymbolKind::Variable => scope
                .var_declarations
                .get(&identifier.raw)
                .cloned()
                .zip(Some(SymbolKind::Variable)),
            SymbolKind::Function => scope
                .function_declarations
                .get(&identifier.raw)
                .cloned()
                .zip(Some(SymbolKind::Function)),
            SymbolKind::Type => scope
                .type_declarations
                .get(&identifier.raw)
                .cloned()
                .zip(Some(SymbolKind::Type)),
            SymbolKind::Abi => scope
                .abi_declarations
                .get(&identifier.raw)
                .cloned()
                .zip(Some(SymbolKind::Abi)),
            SymbolKind::Namespace => scope
                .abi_declarations
                .get(&identifier.raw)
                .cloned()
                .zip(Some(SymbolKind::Abi))
                .or_else(|| {
                    scope
                        .type_declarations
                        .get(&identifier.raw)
                        .cloned()
                        .zip(Some(SymbolKind::Type))
                }),
        });

        let Some((resolved_name, symbol_kind)) = resolution else {
            self.push_not_found_error(identifier.span.unwrap());
            return None;
        };

        identifier.uid.replace(resolved_name);

        Some((resolved_name, symbol_kind))
    }

    fn visit_block(&mut self, block: &mut Block, new_scope: bool) {
        // Blocks as syntax elements can be both part of expressions or just
        // function definitions. We could create an inner scope for the function
        // definition, but it's probably better to not increase depth
        if new_scope {
            self.push_scope();
        }

        let mut curr = block;

        loop {
            match curr {
                Block::Chain { head, tail } => {
                    match &mut **head {
                        ExprOrStatement::Expr(expr) => {
                            self.visit_expr(expr);
                        }
                        ExprOrStatement::Statement(statement) => {
                            self.visit_statement(statement);
                        }
                    }

                    curr = tail;
                }
                Block::Close { semicolon: _ } => {
                    if new_scope {
                        self.pop_scope();
                    }

                    break;
                }
            }
        }
    }

    fn visit_expr(&mut self, expr: &mut Spanned<Expr>) {
        match &mut expr.node {
            Expr::PrimaryExpr(secondary) => {
                self.visit_secondary_expr(secondary);
            }
            Expr::BlockExpr(block_expr) => match block_expr {
                BlockExpr::IfThenElse(cond, _if, _else) => {
                    self.visit_expr(&mut *cond);
                    self.visit_block(&mut *_if, true);
                    if let Some(_else) = _else {
                        self.visit_block(&mut *_else, true);
                    }
                }
                BlockExpr::Block(block) => {
                    self.visit_block(block, true);
                }
            },
            Expr::Equals(lhs, rhs)
            | Expr::NotEquals(lhs, rhs)
            | Expr::LessThan(lhs, rhs)
            | Expr::GreaterThan(lhs, rhs)
            | Expr::LessEq(lhs, rhs)
            | Expr::GreaterEq(lhs, rhs)
            | Expr::Add(lhs, rhs)
            | Expr::Sub(lhs, rhs)
            | Expr::Mul(lhs, rhs)
            | Expr::Div(lhs, rhs)
            | Expr::Mod(lhs, rhs)
            | Expr::BitAnd(lhs, rhs)
            | Expr::BitOr(lhs, rhs)
            | Expr::BitXor(lhs, rhs)
            | Expr::LShift(lhs, rhs)
            | Expr::And(lhs, rhs)
            | Expr::Or(lhs, rhs)
            | Expr::RShift(lhs, rhs) => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            Expr::Neg(expr) | Expr::BitNot(expr) | Expr::Not(expr) => {
                self.visit_expr(expr);
            }
        }
    }

    fn visit_statement(&mut self, stmt: &mut Statement) {
        match stmt {
            Statement::BindVar {
                var,
                mutable,
                value,
                ty,
            } => {
                self.visit_expr(value);
                self.push_var_declaration(var, *mutable);

                if let Some(ty) = ty {
                    self.visit_type_arg(ty);
                }
            }
            Statement::Return(expr) | Statement::Resume(expr) => {
                if let Some(expr) = expr {
                    self.visit_expr(expr)
                }
            }
            Statement::Assign { var, expr } => {
                self.visit_secondary_expr(var);

                self.visit_expr(expr);
            }
            Statement::With(block, items) => {
                self.push_scope();

                for (decl, body) in items {
                    let mut namespace = [&mut decl.interface];
                    self.resolve_name_in_namespace(&mut namespace, &mut decl.ident);

                    let mut identifier =
                        Identifier::new(format!("{}_handle", decl.ident.raw), None);

                    self.push_function_declaration(
                        &mut identifier,
                        FuncInfo {
                            // TODO: check that this matches the storage declaration
                            inputs_ty: vec![],
                            output_ty: None,
                            effects: EffectSet::empty(),
                            locals: vec![],
                            ..Default::default()
                        },
                    );

                    // TODO: depending on whether we compile effect handlers as
                    // functions or not we may need to change this
                    // also to handle captures probably
                    self.push_function_scope(identifier.uid.unwrap());

                    for node in &mut decl.args {
                        self.push_var_declaration(&mut node.name, false);
                    }

                    self.visit_block(body, false);

                    self.pop_scope();
                }

                self.visit_block(block, false);

                self.pop_scope();
            }
            Statement::While(expr, loop_body) => {
                self.visit_expr(expr);
                self.visit_loop_body(loop_body);
            }
            Statement::Loop(loop_body) => {
                self.visit_loop_body(loop_body);
            }
        }
    }

    fn visit_loop_body(&mut self, loop_body: &mut LoopBody) {
        match loop_body {
            LoopBody::Statement(stmt) => self.visit_statement(stmt),
            LoopBody::Block(block) => self.visit_block(block, true),
            LoopBody::Expr(expr) => self.visit_expr(expr),
        }
    }

    fn visit_secondary_expr(&mut self, expr: &mut FieldAccessExpression) {
        match expr {
            FieldAccessExpression::PrimaryExpr(primary_expr) => {
                self.visit_primary_expr(primary_expr)
            }
            FieldAccessExpression::FieldAccess { base, field } => {
                for arg in field.args.iter_mut().flat_map(|args| args.xs.iter_mut()) {
                    self.visit_expr(arg);
                }
                self.visit_secondary_expr(&mut *base);
            }
        }
    }

    fn visit_primary_expr(&mut self, expr: &mut PrimaryExpr) {
        match expr {
            PrimaryExpr::Number(_) => (),
            PrimaryExpr::Bool(_) => (),
            PrimaryExpr::Ident(ident) | PrimaryExpr::Raise { ident } => {
                self.resolve_name(
                    &mut ident.name,
                    if ident.args.is_some() {
                        SymbolKind::Function
                    } else {
                        SymbolKind::Variable
                    },
                );

                if let Some(args) = &mut ident.args {
                    for expr in &mut args.xs {
                        self.visit_expr(expr);
                    }
                }
            }
            PrimaryExpr::Namespace { namespaces, ident }
            | PrimaryExpr::RaiseNamespaced { namespaces, ident } => {
                self.resolve_name_in_namespace(namespaces, &mut ident.name);

                // TODO: duplicated
                if let Some(args) = &mut ident.args {
                    for expr in &mut args.xs {
                        self.visit_expr(expr);
                    }
                }
            }
            PrimaryExpr::ParExpr(expr) => self.visit_expr(expr),
            PrimaryExpr::Yield(expr) => {
                if let Some(expr) = expr {
                    self.visit_expr(expr)
                }
            }
            PrimaryExpr::Object(_, items) => {
                for (_ident, item) in items {
                    self.visit_expr(item);
                }
            }
            PrimaryExpr::StringLiteral(_) => (),
            PrimaryExpr::Tuple(vals) => {
                for val in vals {
                    self.visit_expr(val);
                }
            }
        }
    }

    fn resolve_name_in_namespace<T>(&mut self, namespaces: &mut [T], ident: &mut Identifier)
    where
        T: AsMut<Identifier>,
    {
        let mut last_namespace = None;

        for namespace in namespaces {
            if let Some(namespace) = self.resolve_name(namespace.as_mut(), SymbolKind::Namespace) {
                last_namespace.replace(namespace);
            }
        }

        let Some((namespace, kind)) = last_namespace else {
            return;
        };

        let f = match kind {
            SymbolKind::Type => self
                .symbols
                .types
                .get(&namespace)
                .unwrap()
                .info
                .declarations
                .iter(),
            SymbolKind::Abi => self
                .symbols
                .interfaces
                .get(&namespace)
                .unwrap()
                .info
                .effects
                .iter(),
            _ => unreachable!(),
        }
        .find(|uid| {
            self.symbols
                .functions
                .get(uid)
                .map(|finfo| finfo.source == ident.raw)
                .unwrap_or(false)
        });

        if let Some(f) = f {
            ident.uid.replace(*f);
        } else {
            self.push_not_found_error(ident.span.unwrap());
        }
    }

    fn visit_abi(&mut self, abi: &mut Abi) {
        let mut effects = HashSet::new();
        let mut fns = HashMap::new();

        for item in &mut abi.values {
            match item {
                AbiElem::FnDecl(decl) => {
                    for ty in &mut decl.0.input_types {
                        self.visit_type_arg(ty);
                    }

                    if let Some(output_ty) = &mut decl.0.output_type {
                        self.visit_type_arg(output_ty);
                    }

                    fns.insert(decl.0.name.raw.clone(), decl.0.clone());
                }
                AbiElem::EffectDecl(decl) => match decl {
                    EffectDecl::EffectSig(decl)
                    | EffectDecl::EventSig(decl)
                    | EffectDecl::ErrorSig(decl) => {
                        let symbol = self.new_symbol(&mut decl.name);

                        self.symbols.functions.insert(
                            symbol,
                            SymbolInformation {
                                source: decl.name.raw.clone(),
                                span: decl.name.span,
                                info: FuncInfo {
                                    inputs_ty: decl.input_types.clone(),
                                    output_ty: decl.output_type.clone(),
                                    effects: EffectSet::empty(),
                                    locals: vec![],
                                    ..Default::default()
                                },
                            },
                        );

                        effects.insert(symbol);
                    }
                },
            }
        }

        self.push_interface_declaration(&mut abi.name, AbiInfo { effects, fns });
    }

    fn visit_type_arg(&mut self, ty: &mut TypeArg) {
        match ty {
            TypeArg::Unit => (),
            TypeArg::Bool => (),
            TypeArg::String => (),
            TypeArg::F32 => (),
            TypeArg::F64 => (),
            TypeArg::U32 => (),
            TypeArg::I32 => (),
            TypeArg::U64 => (),
            TypeArg::I64 => (),
            TypeArg::Intermediate { abi, storage } => {
                self.visit_type_arg(abi);
                self.visit_type_arg(storage);
            }
            TypeArg::TypeRef(type_ref) => {
                self.resolve_name(&mut type_ref.0, SymbolKind::Type);
            }
            TypeArg::TypeApplication(type_ref, params) => {
                self.resolve_name(&mut type_ref.0, SymbolKind::Type);

                for param in params {
                    self.visit_type_arg(param);
                }
            }
            TypeArg::FnType(FnType { inputs, output }) => {
                for (_, ty) in &mut inputs.values {
                    self.visit_type_arg(ty);
                }

                if let Some(output_ty) = output {
                    self.visit_type_arg(output_ty);
                }
            }
            TypeArg::Ref(type_arg) => self.visit_type_arg(type_arg),
        }
    }

    fn push_not_found_error(&mut self, span: SimpleSpan) {
        self.errors.push(
            Report::build(ReportKind::Error, span.into_range())
                .with_config(ariadne::Config::new().with_index_type(ariadne::IndexType::Byte))
                // TODO: define error codes across the compiler
                .with_code(1)
                .with_label(
                    Label::new(span.into_range())
                        .with_message("not found in this scope")
                        .with_color(Color::Red),
                )
                .finish(),
        );
    }

    fn push_redeclaration_error(&mut self, prev: SimpleSpan, new: SimpleSpan) {
        self.errors.push(
            Report::build(ReportKind::Error, new.into_range())
                .with_config(ariadne::Config::new().with_index_type(ariadne::IndexType::Byte))
                // TODO: define error codes across the compiler
                .with_code(2)
                .with_label(
                    Label::new(new.into_range())
                        .with_message("function already declared")
                        .with_color(Color::Red),
                )
                .with_label(
                    Label::new(prev.into_range())
                        .with_message("here")
                        .with_color(Color::BrightRed),
                )
                .finish(),
        );
    }

    fn push_abi_mismatch_error(&mut self, def_span: SimpleSpan, abi_span: SimpleSpan) {
        self.errors.push(
            Report::build(ReportKind::Error, def_span.into_range())
                .with_config(ariadne::Config::new().with_index_type(ariadne::IndexType::Byte))
                // TODO: define error codes across the compiler
                .with_code(4)
                .with_label(
                    Label::new(def_span.into_range())
                        .with_message("function definition doesn't match abi")
                        .with_color(Color::Red),
                )
                .with_label(
                    Label::new(abi_span.into_range())
                        .with_message("defined here")
                        .with_color(Color::Green),
                )
                .finish(),
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{TypeArg, TypeRef};

    use super::do_scope_analysis;
    use ariadne::Source;
    use chumsky::Parser as _;

    #[test]
    fn resolve_usdc_example() {
        let input = include_str!("../../grammar/examples/permissioned_usdc.star");
        let program = crate::starstream_program().parse(input).unwrap();

        // dbg!(&program);

        let ast = do_scope_analysis(program);

        if let Err(errors) = ast {
            for e in errors {
                e.print(Source::from(input)).unwrap();
            }

            panic!();
        }
    }

    #[test]
    fn resolve_oracle_example() {
        let input = include_str!("../../grammar/examples/oracle.star");
        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        if let Err(errors) = ast {
            for e in errors {
                e.print(Source::from(input)).unwrap();
            }

            panic!();
        }
    }

    #[test]
    fn resolve_abi_undeclared_fails() {
        let input = "
            abi Abi {
                fn foo(): u32;
            }

            utxo Utxo {
                impl Abi {
                    fn bar(self) {}
                }
            }
        ";

        let ast = do_scope_analysis(crate::starstream_program().parse(input).unwrap());

        assert!(ast.is_err());

        let input = "
            abi Abi {
                fn foo(): u32;
            }

            utxo Utxo {
                impl Abi {
                    fn foo(self): u64 {}
                }
            }
        ";

        let ast = do_scope_analysis(crate::starstream_program().parse(input).unwrap());

        assert!(ast.is_err());

        let input = "
            abi Abi {
                fn foo(): u32;
            }

            utxo Utxo {
                impl Abi {
                    fn foo(self): u32 {}
                }
            }
        ";

        let ast = do_scope_analysis(crate::starstream_program().parse(input).unwrap());

        assert!(ast.is_ok());
    }

    #[test]
    fn unbound_variable_fails() {
        let input = "
            script {
              fn foo() {
                let x = y + 1;
              }
            }
        ";

        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        assert!(ast.is_err());

        let input = "
            script {
              fn foo(y: u32) {
                let x = y + 1;
              }
            }
        ";

        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        assert!(ast.is_ok());
    }

    #[test]
    fn shadowing() {
        let input = "
            script {
              fn foo() {
                let mut x = 5;
                let y = 42;
                let x = x + y;

                x + x;
              }
            }
        ";

        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        match ast {
            Err(_errors) => {
                unreachable!();
            }
            Ok((_ast, table)) => {
                let vars = table
                    .vars
                    .values()
                    .filter(|info| info.source == "x")
                    .collect::<Vec<_>>();

                assert_eq!(vars.len(), 2);

                let first = vars.iter().find(|info| info.info.index == 0).unwrap();

                let second = vars.iter().find(|info| info.info.index == 2).unwrap();

                assert!(first.info.mutable);
                assert!(!second.info.mutable);

                assert_eq!(table.vars.len(), 3);
            }
        }
    }

    #[test]
    fn script_function_order() {
        let input = "
            script {
              fn foo() {
                  bar();
              }

              fn bar() {}
            }
        ";

        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        match ast {
            Err(_errors) => {
                for e in _errors {
                    e.eprint(Source::from(input)).unwrap();
                }
                unreachable!();
            }
            Ok((_ast, _table)) => {}
        }
    }

    #[test]
    fn script_function_same_name_fails() {
        let input = "
            script {
              fn foo() {
              }

              fn foo() {
              }
            }
        ";

        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        match ast {
            Err(_errors) => {
                // for e in _errors {
                //     e.eprint(Source::from(input)).unwrap();
                // }
            }
            Ok((_ast, _table)) => {
                unreachable!();
            }
        }
    }

    #[test]
    fn function_type_extraction() {
        let input = "
            abi Abi {
                effect Effect1(bool): u32;
            }

            token MyToken {
                mint {}
            }

            script {
              fn foo(): u32 / { Abi } {}

              fn bar(i: u64): bool {}

              fn handler() {
                try {}
                with Abi::Effect1(x) { yield 4; }
              }
            }
        ";

        let program = crate::starstream_program().parse(input).unwrap();

        let ast = do_scope_analysis(program);

        match ast {
            Err(_errors) => {
                for e in _errors {
                    e.eprint(Source::from(input)).unwrap();
                }
                panic!();
            }
            Ok((_ast, table)) => {
                // unreachable!();
                // dbg!(&table.functions);
                // panic!();
                //
                let eff = table
                    .functions
                    .values()
                    .find(|f| f.source == "Effect1")
                    .unwrap();

                assert_eq!(eff.info.inputs_ty, vec![TypeArg::Bool]);
                assert_eq!(eff.info.output_ty.clone().unwrap(), TypeArg::U32);

                let foo = table
                    .functions
                    .values()
                    .find(|f| f.source == "foo")
                    .unwrap();

                assert_eq!(foo.info.inputs_ty, vec![]);
                assert_eq!(foo.info.output_ty.clone().unwrap(), TypeArg::U32);

                let bar = table
                    .functions
                    .values()
                    .find(|f| f.source == "bar")
                    .unwrap();

                assert_eq!(bar.info.inputs_ty, vec![TypeArg::U64]);
                assert_eq!(bar.info.output_ty.clone().unwrap(), TypeArg::Bool);

                let mint = table
                    .functions
                    .values()
                    .find(|f| f.source == "mint")
                    .unwrap();

                assert_eq!(mint.info.inputs_ty, vec![]);

                let TypeArg::TypeRef(TypeRef(ident)) = mint.info.output_ty.clone().unwrap() else {
                    panic!()
                };

                assert_eq!(ident.raw, "MyToken");
                assert_eq!(table.types[&ident.uid.unwrap()].source, "MyToken");
            }
        }
    }
}
