use crate::ast::{FnType, *};
use ariadne::{Color, Label, Report, ReportKind};
use chumsky::{
    pratt::{infix, left, prefix},
    prelude::*,
};

/// Convert a Chumsky parse error to a fancy diagnostic report.
pub fn error_to_report(e: Rich<char>) -> Report {
    Report::build(ReportKind::Error, e.span().into_range())
        .with_config(ariadne::Config::new().with_index_type(ariadne::IndexType::Byte))
        .with_message(e.to_string())
        .with_label(
            Label::new(e.span().into_range())
                .with_message(e.reason().to_string())
                .with_color(Color::Red),
        )
        .finish()
}

/// Get a Chumsky parser for a Starstream source file.
pub fn starstream_program<'a>()
-> impl Parser<'a, &'a str, StarstreamProgram, extra::Err<Rich<'a, char>>> {
    utxo()
        .map(ProgramItem::Utxo)
        .or(script().map(ProgramItem::Script))
        .or(token().map(ProgramItem::Token))
        .or(typedef().map(ProgramItem::TypeDef))
        .or(constant().map(|(name, value)| ProgramItem::Constant { name, value }))
        .or(abi().map(ProgramItem::Abi))
        .padded()
        .repeated()
        .collect::<Vec<_>>()
        .then_ignore(end())
        .map(|items| StarstreamProgram { items })
}

fn utxo<'a>() -> impl Parser<'a, &'a str, Utxo, extra::Err<Rich<'a, char>>> {
    just("utxo")
        .ignore_then(identifier().padded())
        .then(
            main()
                .map(UtxoItem::Main)
                .or(r#impl().map(UtxoItem::Impl))
                .or(storage().map(UtxoItem::Storage))
                .or(just("Yield")
                    .padded()
                    .ignore_then(type_arg())
                    .map(|ty| UtxoItem::Yield(ty)))
                .or(just("Resume")
                    .padded()
                    .ignore_then(type_arg())
                    .map(|ty| UtxoItem::Resume(ty)))
                .padded()
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|(name, items)| Utxo { name, items })
}

fn fn_sig<'a>() -> impl Parser<'a, &'a str, FnDecl, extra::Err<Rich<'a, char>>> {
    just("fn").ignore_then(sig()).map(FnDecl)
}

fn effect_sig<'a>() -> impl Parser<'a, &'a str, EffectDecl, extra::Err<Rich<'a, char>>> {
    choice((
        just("effect").ignore_then(sig()).map(EffectDecl::EffectSig),
        just("event").ignore_then(sig()).map(EffectDecl::EventSig),
        just("error").ignore_then(sig()).map(EffectDecl::ErrorSig),
    ))
}

fn sig<'a>() -> impl Parser<'a, &'a str, Sig, extra::Err<Rich<'a, char>>> {
    identifier()
        .padded()
        .then(
            type_arg()
                .padded()
                .separated_by(just(','))
                .collect::<Vec<_>>()
                .delimited_by(just('('), just(')')),
        )
        .then(just(':').ignore_then(type_arg().padded()).or_not())
        .map(|((name, input_types), output_type)| Sig {
            name,
            input_types,
            output_type,
        })
}

fn fn_def<'a>() -> impl Parser<'a, &'a str, FnDef, extra::Err<Rich<'a, char>>> {
    let typed_bindings = typed_binding(type_arg())
        .map(|(name, ty)| FnArgDeclaration {
            name,
            ty: TypeOrSelf::Type(ty),
        })
        .or(just("self").map_with(|ident, extra| FnArgDeclaration {
            name: Identifier::new(ident, Some(extra.span())),
            ty: TypeOrSelf::_Self,
        }))
        .separated_by(just(',').padded())
        .allow_trailing()
        .collect::<Vec<_>>()
        .boxed();

    just("fn")
        .padded()
        .ignore_then(identifier())
        .padded()
        .then(typed_bindings.padded().delimited_by(just('('), just(')')))
        .then(just(':').ignore_then(type_arg().padded()).or_not())
        .then(
            just('/')
                .padded()
                .then_ignore(just('{').padded())
                .ignore_then(
                    identifier()
                        .separated_by(just(',').padded())
                        .collect::<Vec<_>>(),
                )
                .then_ignore(just('}').padded())
                .or_not(),
        )
        .then(block())
        .map(|((((name, inputs), output), effects), body)| FnDef {
            ident: name,
            inputs,
            output,
            body,
            effects: effects.unwrap_or_default(),
        })
}

fn token<'a>() -> impl Parser<'a, &'a str, Token, extra::Err<Rich<'a, char>>> {
    just("token")
        .padded()
        .ignore_then(identifier())
        .then(
            just("bind")
                .padded()
                .ignore_then(block())
                .map(Bind)
                .map(TokenItem::Bind)
                .or(just("unbind")
                    .padded()
                    .ignore_then(block())
                    .map(Unbind)
                    .map(TokenItem::Unbind))
                .or(just("mint")
                    .padded()
                    .ignore_then(block())
                    .map(Mint)
                    .map(TokenItem::Mint))
                .padded()
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|(name, items)| Token { name, items })
}

fn r#impl<'a>() -> impl Parser<'a, &'a str, Impl, extra::Err<Rich<'a, char>>> {
    just("impl")
        .padded()
        .ignore_then(identifier())
        .then(
            fn_def()
                .padded()
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|(name, definitions)| Impl { name, definitions })
}

fn script<'a>() -> impl Parser<'a, &'a str, Script, extra::Err<Rich<'a, char>>> {
    just("script")
        .padded()
        .ignore_then(
            fn_def()
                .padded()
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|definitions| Script { definitions })
}

fn abi<'a>() -> impl Parser<'a, &'a str, Abi, extra::Err<Rich<'a, char>>> {
    just("abi")
        .ignore_then(identifier().padded())
        .then(
            choice((
                fn_sig().map(AbiElem::FnDecl),
                effect_sig().map(AbiElem::EffectDecl),
            ))
            .then_ignore(just(';').padded())
            .repeated()
            .collect::<Vec<_>>()
            .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|(name, values)| Abi { name, values })
}

fn storage<'a>() -> impl Parser<'a, &'a str, Storage, extra::Err<Rich<'a, char>>> {
    just("storage")
        .ignore_then(
            typed_binding(type_arg())
                .then_ignore(just(';').padded())
                .repeated()
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|values| Storage {
            bindings: TypedBindings { values },
        })
}

fn main<'a>() -> impl Parser<'a, &'a str, Main, extra::Err<Rich<'a, char>>> {
    just("main")
        .ignore_then(
            typed_binding(type_arg())
                .separated_by(just(',').padded())
                .collect::<Vec<_>>()
                .map(|values| TypedBindings { values })
                .delimited_by(just('(').padded(), just(')').padded())
                .or_not(),
        )
        .then(block())
        .map(|(typed_bindings, block)| Main {
            type_sig: typed_bindings,
            block,
            ident: Identifier::new("new", None),
        })
}

fn statement<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
    block_parser: impl Parser<'a, &'a str, Block, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, Statement, extra::Err<Rich<'a, char>>> {
    recursive(|rec| {
        let bind_var = just("let")
            .padded()
            .ignore_then(just("mut").padded().or_not().map(|x| x.is_some()))
            .then(identifier())
            .then(just(":").padded().ignore_then(type_arg()).or_not())
            .then_ignore(just('=').padded())
            .then(expr_parser.clone())
            .then_ignore(just(';').padded())
            .map(|(((mutable, binding), ty), expr)| Statement::BindVar {
                var: binding,
                ty,
                value: expr,
                mutable,
            })
            .boxed();

        let assign = field_access_expr(expr_parser.clone())
            .then_ignore(just('=').padded())
            .then(expr_parser.clone())
            .then_ignore(just(';'))
            .map(|(var, expr)| Statement::Assign { var, expr })
            .boxed();

        let loop_body = rec
            .clone()
            .map(Box::new)
            .map(LoopBody::Statement)
            .or(block_parser.clone().map(LoopBody::Block))
            .or(expr_parser
                .clone()
                .then_ignore(just(';'))
                .map(LoopBody::Expr));

        let while_statement = just("while")
            .padded()
            .ignore_then(
                expr_parser
                    .clone()
                    .delimited_by(just('(').padded(), just(')').padded()),
            )
            .then(loop_body.clone())
            .map(|(cond, body)| Statement::While(cond, body))
            .boxed();

        let loop_statement = just("loop")
            .padded()
            .ignore_then(loop_body)
            .map(Statement::Loop)
            .boxed();

        let try_with = just("try")
            .ignore_then(block_parser.clone())
            .then(
                just("with")
                    .ignore_then(effect_handler().padded())
                    .then(block_parser.clone().padded())
                    .repeated()
                    .collect::<Vec<_>>(),
            )
            .map(|(body, handlers)| Statement::With(body, handlers))
            .boxed();

        let resume = just("resume")
            .ignore_then(expr_parser.clone().padded().or_not())
            .then_ignore(just(';').padded())
            .map(Statement::Resume);

        let ret = just("return")
            .ignore_then(expr_parser.clone().padded().or_not())
            .then_ignore(just(';').padded())
            .map(Statement::Return);

        choice((
            bind_var,
            assign,
            while_statement,
            loop_statement,
            try_with,
            resume,
            ret,
        ))
    })
}

fn effect_handler<'a>() -> impl Parser<'a, &'a str, EffectHandler, extra::Err<Rich<'a, char>>> {
    identifier()
        .then_ignore(just("::"))
        .then(
            identifier()
                .then(optionally_typed_bindings(type_arg()).delimited_by(just('('), just(')'))),
        )
        .map(|(utxo, (ident, args))| EffectHandler {
            interface: utxo,
            ident,
            args: args
                .values
                .into_iter()
                .map(|(name, ty)| EffectArgDeclaration { name, ty })
                .collect(),
        })
}

fn typed_binding<'a>(
    type_parser: impl Parser<'a, &'a str, TypeArg, extra::Err<Rich<'a, char>>>,
) -> impl Parser<'a, &'a str, (Identifier, TypeArg), extra::Err<Rich<'a, char>>> {
    identifier().then(just(':').padded().ignore_then(type_parser.padded()))
}

fn optionally_typed_binding<'a>(
    type_parser: impl Parser<'a, &'a str, TypeArg, extra::Err<Rich<'a, char>>>,
) -> impl Parser<'a, &'a str, (Identifier, Option<TypeArg>), extra::Err<Rich<'a, char>>> {
    identifier().then(
        just(':')
            .padded()
            .ignore_then(type_parser.padded())
            .or_not(),
    )
}

fn optionally_typed_bindings<'a>(
    type_parser: impl Parser<'a, &'a str, TypeArg, extra::Err<Rich<'a, char>>>,
) -> impl Parser<'a, &'a str, OptionallyTypedBindings, extra::Err<Rich<'a, char>>> {
    optionally_typed_binding(type_parser)
        .separated_by(just(',').padded())
        .allow_trailing()
        .collect::<Vec<_>>()
        .map(|values| OptionallyTypedBindings { values })
}

fn expr<'a>(
    block_parser: impl Parser<'a, &'a str, Block, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> {
    let op = |c: &'static str| just(c).padded();

    recursive(|expr_parser| {
        let atom = field_access_expr(expr_parser.clone())
            .map_with(|e, extra| Spanned {
                node: Expr::PrimaryExpr(e),
                span: extra.span(),
            })
            .or(
                block_expr(expr_parser, block_parser).map_with(|e, extra| Spanned {
                    node: Expr::BlockExpr(e),
                    span: extra.span(),
                }),
            );

        atom.pratt((
            // prec = 10
            prefix(10, op("-"), |_, atom, extra| Spanned {
                node: Expr::Neg(Box::new(atom)),
                span: extra.span(),
            }),
            prefix(10, op("!"), |_, atom, extra| Spanned {
                node: Expr::Not(Box::new(atom)),
                span: extra.span(),
            }),
            prefix(10, op("~"), |_, atom, extra| Spanned {
                node: Expr::BitNot(Box::new(atom)),
                span: extra.span(),
            }),
            // prec = 9
            infix(left(9), op("*"), |l, _, r, extra| Spanned {
                node: Expr::Mul(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(9), op("/"), |l, _, r, extra| Spanned {
                node: Expr::Div(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(9), op("%"), |l, _, r, extra| Spanned {
                node: Expr::Div(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 8
            infix(left(8), op("+"), |l, _, r, extra| Spanned {
                node: Expr::Add(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(8), op("-"), |l, _, r, extra| Spanned {
                node: Expr::Sub(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 7
            infix(left(7), op("<<"), |l, _, r, extra| Spanned {
                node: Expr::LShift(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(7), op(">>"), |l, _, r, extra| Spanned {
                node: Expr::RShift(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 6
            infix(left(6), op("<"), |l, _, r, extra| Spanned {
                node: Expr::LessThan(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(6), op(">"), |l, _, r, extra| Spanned {
                node: Expr::GreaterThan(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(6), op("<="), |l, _, r, extra| Spanned {
                node: Expr::LessEq(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(6), op(">="), |l, _, r, extra| Spanned {
                node: Expr::LessThan(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 5
            infix(left(5), op("=="), |l, _, r, extra| Spanned {
                node: Expr::Equals(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            infix(left(5), op("!="), |l, _, r, extra| Spanned {
                node: Expr::NotEquals(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 4
            infix(left(4), op("&"), |l, _, r, extra| Spanned {
                node: Expr::BitAnd(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 3
            infix(left(3), op("^"), |l, _, r, extra| Spanned {
                node: Expr::BitXor(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 2
            infix(left(2), op("|"), |l, _, r, extra| Spanned {
                node: Expr::BitOr(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 1
            infix(left(1), op("&&"), |l, _, r, extra| Spanned {
                node: Expr::And(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
            // prec = 0
            infix(left(0), just("||").padded(), |l, _, r, extra| Spanned {
                node: Expr::Or(Box::new(l), Box::new(r)),
                span: extra.span(),
            }),
        ))
        .boxed()
    })
}

fn block<'a>() -> impl Parser<'a, &'a str, Block, extra::Err<Rich<'a, char>>> {
    let mut block_expr = Recursive::declare();
    let mut block_body = Recursive::declare();

    let expr_parser = expr(block_expr.clone()).boxed();

    block_body.define({
        let end_block = just(';')
            .padded()
            .or_not()
            .then_ignore(comment().boxed().ignore_then(just('}').padded()))
            .map(|semicolon| Block::Close {
                semicolon: semicolon.is_some(),
            });

        let if_branch = if_expr(expr_parser.clone(), block_expr.clone())
            .padded()
            .map_with(|block, extra| Spanned {
                node: Expr::BlockExpr(block),
                span: extra.span(),
            })
            .map(ExprOrStatement::Expr)
            .then(end_block.clone().or(block_body.clone()))
            .padded();

        let expr_with_semicolon = expr_parser
            .clone()
            .padded()
            .map(ExprOrStatement::Expr)
            .then(
                end_block.clone().or(just(";")
                    .ignored()
                    .padded()
                    .ignore_then(block_body.clone())
                    .padded()),
            );

        let statement = statement(expr_parser.clone(), block_expr.clone())
            .padded()
            .map(ExprOrStatement::Statement)
            .then(block_body.clone().padded().or(end_block))
            .boxed();

        let block_body_item = just('}')
            .to(Block::Close { semicolon: false })
            .padded()
            .or(
                choice((if_branch, expr_with_semicolon, statement)).map(|(x, xs)| Block::Chain {
                    head: Box::new(x),
                    tail: Box::new(xs),
                }),
            );

        comment().boxed().ignore_then(block_body_item)
    });

    block_expr.define(just('{').padded().ignore_then(block_body));

    block_expr
}

fn block_expr<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
    block_parser: impl Parser<'a, &'a str, Block, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, BlockExpr, extra::Err<Rich<'a, char>>> {
    let parse_block = block_parser.clone().map(BlockExpr::Block);
    let if_expr = if_expr(expr_parser.clone(), block_parser.clone());

    choice((parse_block, if_expr))
}

fn if_expr<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
    block_parser: impl Parser<'a, &'a str, Block, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, BlockExpr, extra::Err<Rich<'a, char>>> + Clone {
    just("if")
        .ignore_then(
            expr_parser
                .clone()
                .padded()
                .delimited_by(just("(").padded(), just(")").padded()),
        )
        .then(block_parser.clone().padded())
        .then(
            just("else")
                .padded()
                .ignore_then(block_parser.padded())
                .or_not(),
        )
        .map(|((expr1, expr2), expr3)| {
            BlockExpr::IfThenElse(Box::new(expr1), Box::new(expr2), expr3.map(Box::new))
        })
        .labelled("if-expr")
        .boxed()
}

fn field_access_expr<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, FieldAccessExpression, extra::Err<Rich<'a, char>>> {
    primary_expr(expr_parser.clone())
        .map(FieldAccessExpression::PrimaryExpr)
        .foldl(
            just('.')
                .padded()
                .ignore_then(identifier_expr(expr_parser))
                .repeated(),
            |accum, new| FieldAccessExpression::FieldAccess {
                base: Box::new(accum),
                field: new,
            },
        )
}

fn application<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, Arguments, extra::Err<Rich<'a, char>>> {
    expr_parser
        .clone()
        .separated_by(just(',').padded())
        .allow_trailing()
        .collect::<Vec<_>>()
        .map(|xs| Arguments { xs })
        .delimited_by(just('('), just(')'))
}

fn identifier_expr<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, IdentifierExpr, extra::Err<Rich<'a, char>>> {
    identifier()
        .then(application(expr_parser).or_not())
        .map(|(name, args)| IdentifierExpr { name, args })
}

fn primary_expr<'a>(
    expr_parser: impl Parser<'a, &'a str, Spanned<Expr>, extra::Err<Rich<'a, char>>> + Clone + 'a,
) -> impl Parser<'a, &'a str, PrimaryExpr, extra::Err<Rich<'a, char>>> {
    let number = just('-')
        .or_not()
        .then(text::int(10))
        .to_slice()
        .map(|s: &str| s.parse().unwrap())
        .map(PrimaryExpr::Number);

    let bool = choice((
        just("true").to(PrimaryExpr::Bool(true)),
        just("false").to(PrimaryExpr::Bool(false)),
    ));

    let par_expr = expr_parser
        .clone()
        .padded()
        .delimited_by(just('('), just(')'))
        .map(|expr| PrimaryExpr::ParExpr(Box::new(expr)));

    let yield_expr = just("yield")
        .ignore_then(expr_parser.clone().padded().map(Box::new).or_not())
        .map(PrimaryExpr::Yield);

    let object = type_arg()
        .then(
            identifier()
                .then_ignore(just(":"))
                .then(expr_parser.clone().padded())
                .separated_by(just(',').padded())
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|(ty, values)| PrimaryExpr::Object(ty, values));

    let ident = identifier()
        .map(|i| vec![i])
        .foldl(
            just("::").ignore_then(identifier()).repeated(),
            |mut accum, new| {
                accum.push(new);
                accum
            },
        )
        .then(application(expr_parser.clone()).or_not())
        .map(|(mut idents, args)| {
            let ident = IdentifierExpr {
                name: idents.pop().unwrap(),
                args,
            };

            if idents.is_empty() {
                PrimaryExpr::Ident(ident)
            } else {
                PrimaryExpr::Namespace {
                    namespaces: idents,
                    ident,
                }
            }
        });

    let raise_expr = just("raise").padded().ignore_then(
        identifier()
            .map(|i| vec![i])
            .foldl(
                just("::").ignore_then(identifier()).repeated(),
                |mut accum, new| {
                    accum.push(new);
                    accum
                },
            )
            .then(application(expr_parser.clone()))
            .map(|(mut idents, args)| {
                let ident = IdentifierExpr {
                    name: idents.pop().unwrap(),
                    args: Some(args),
                };

                if idents.is_empty() {
                    PrimaryExpr::Raise { ident }
                } else {
                    PrimaryExpr::RaiseNamespaced {
                        namespaces: idents,
                        ident,
                    }
                }
            }),
    );

    let string_literal = none_of('"')
        .repeated()
        .collect::<String>()
        .padded_by(just('"'))
        .map(PrimaryExpr::StringLiteral);

    let tuple = expr_parser
        .separated_by(just(',').padded())
        .collect::<Vec<_>>()
        .delimited_by(just('(').padded(), just(')').padded())
        .map(|vals| PrimaryExpr::Tuple(vals));

    choice((
        number,
        bool,
        par_expr,
        yield_expr,
        raise_expr,
        object,
        ident,
        string_literal,
        tuple,
    ))
    .boxed()
}

fn reserved_word<'a>() -> impl Parser<'a, &'a str, (), extra::Err<Rich<'a, char>>> {
    choice((just("enum"), just("typedef"), just("loop")))
        .padded()
        .ignored()
}

fn identifier<'a>() -> impl Parser<'a, &'a str, Identifier, extra::Err<Rich<'a, char>>> {
    text::ident()
        .and_is(reserved_word().not())
        .map_with(|s: &'a str, extra| Identifier::new(s, Some(extra.span())))
}

fn typedef<'a>() -> impl Parser<'a, &'a str, TypeDef, extra::Err<Rich<'a, char>>> {
    let typed_bindings = typed_binding(r#type_arg())
        .separated_by(just(',').padded())
        .collect::<Vec<_>>()
        .boxed();

    let object = typed_bindings
        .clone()
        .delimited_by(just('{').padded(), just('}').padded())
        .map(|values| TypeDefRhs::Object(TypedBindings { values }))
        .boxed();

    let variant = just("enum")
        .ignore_then(
            identifier()
                .padded()
                .then(
                    typed_bindings
                        .map(|values| TypedBindings { values })
                        .delimited_by(just('(').padded(), just(')').padded()),
                )
                .separated_by(just(',').padded())
                .collect::<Vec<_>>()
                .delimited_by(just('{').padded(), just('}').padded()),
        )
        .map(|values| TypeDefRhs::Variant(Variant(values)))
        .boxed();

    let type_arg = type_arg().map(TypeDefRhs::TypeArg).boxed();

    just("typedef")
        .ignore_then(identifier().padded())
        .then_ignore(just("=").padded())
        .then(choice((object, variant, type_arg)))
        .map(|(name, ty)| TypeDef { name, ty })
}

fn constant<'a>() -> impl Parser<'a, &'a str, (Identifier, f64), extra::Err<Rich<'a, char>>> {
    just("const")
        .ignore_then(identifier().padded())
        .then_ignore(just("=").padded())
        .then(text::int(10).to_slice().map(|s: &str| s.parse().unwrap()))
        .then_ignore(just(";"))
}

fn type_arg<'a>() -> impl Parser<'a, &'a str, TypeArg, extra::Err<Rich<'a, char>>> {
    let mut type_parser = Recursive::declare();

    type_parser.define({
        let bool = just("bool").to(TypeArg::Bool);

        let p_f32 = just("f32").to(TypeArg::F32);
        let p_f64 = just("f64").to(TypeArg::F64);
        let p_u32 = just("u32").to(TypeArg::U32);
        let p_u64 = just("u64").to(TypeArg::U64);
        let p_i32 = just("i32").to(TypeArg::I32);
        let p_i64 = just("i64").to(TypeArg::I64);

        let string = just("string").to(TypeArg::String);

        let intermediate = just("Intermediate")
            .padded()
            .ignore_then(
                type_parser
                    .clone()
                    .map(Box::new)
                    .then_ignore(just(',').padded())
                    .then(type_parser.clone().map(Box::new))
                    .delimited_by(just('<').padded(), just('>').padded())
                    .clone(),
            )
            .map(|(abi, storage)| TypeArg::Intermediate { abi, storage });

        let type_ref = identifier().padded().map(TypeRef).boxed();

        let type_application = type_ref
            .clone()
            .then(
                type_parser
                    .clone()
                    .separated_by(just(',').padded())
                    .collect::<Vec<_>>()
                    .delimited_by(just('<').padded(), just('>').padded()),
            )
            .map(|(base, params)| TypeArg::TypeApplication(base, params))
            .boxed();

        let typed_bindings = typed_binding(type_parser.clone())
            .separated_by(just(',').padded())
            .collect::<Vec<_>>()
            .boxed();

        let fn_type = typed_bindings
            .clone()
            .delimited_by(just('(').padded(), just(')').padded())
            .then(
                just("->")
                    .padded()
                    .ignore_then(type_parser.clone())
                    .or_not(),
            )
            .map(|(inputs, output)| {
                TypeArg::FnType(FnType {
                    inputs: TypedBindings { values: inputs },
                    output: output.map(Box::new),
                })
            })
            .boxed();

        choice((
            bool,
            p_f32,
            p_f64,
            p_u32,
            p_i32,
            p_u64,
            p_i64,
            string,
            intermediate,
            fn_type,
            type_application,
            type_ref.map(TypeArg::TypeRef),
        ))
        .clone()
    });

    type_parser
}

fn comment<'a>() -> impl Parser<'a, &'a str, (), extra::Err<Rich<'a, char>>> {
    just("//")
        .padded()
        .then_ignore(any().and_is(text::newline().not()).repeated())
        .then_ignore(text::newline())
        .repeated()
        .or_not()
        .ignored()
}

#[cfg(test)]
mod tests {
    use super::*;

    use ariadne::Source;

    fn test_with_diagnostics<'a, T: std::fmt::Debug>(
        input: &'a str,
        parser: impl Parser<'a, &'a str, T, extra::Full<Rich<'a, char>, (), ()>>,
    ) -> T {
        let (output, errors) = parser.parse(input).into_output_errors();

        for e in errors {
            error_to_report(e).eprint(Source::from(input)).unwrap();
        }

        dbg!(output.unwrap())
    }

    #[test]
    fn parse_secondary_expr() {
        let input = "foo(1, 2, 3)";
        test_with_diagnostics(input, field_access_expr(expr(block().boxed()).boxed()));

        let input = "foo.x.y(1, 2, 3)";
        test_with_diagnostics(input, field_access_expr(expr(block().boxed()).boxed()));

        let input = r#"foo("bar").x(3, 4).y(1, 2, 3)"#;
        test_with_diagnostics(input, field_access_expr(expr(block().boxed()).boxed()));
    }

    #[test]
    fn parse_expr() {
        let input = "foo.x()";
        test_with_diagnostics(input, expr(block().boxed()));

        let input = "foo()";
        test_with_diagnostics(input, expr(block().boxed()));

        let input = "foo.x.y(3, 4)";
        test_with_diagnostics(input, expr(block().boxed()));

        let input = "Type { x: 4, y: 5 }";
        test_with_diagnostics(input, expr(block().boxed()));

        let input = "Type::func(3)";
        test_with_diagnostics(input, expr(block().boxed()));
    }

    #[test]
    fn parse_main() {
        let input = "main {
            let y: u32 = 5;
            while(true) yield 4 + 4;
            loop { let z = 4; }
            y = 3;
            try { let z = 4; }
            with Token1::Effect1(x: T) { yield 4; }
            with Token2::Effect1(x) { yield x; }
        }";
        test_with_diagnostics(input, main());
    }

    #[test]
    fn parse_block() {
        let input = "{ 4 }";
        let output = test_with_diagnostics(input, block());
        match output {
            Block::Chain { head: _, tail } => match *tail {
                Block::Close { semicolon } => assert!(!semicolon),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }

        let input = "{ 4; }";
        let output = test_with_diagnostics(input, block());

        match output {
            Block::Chain { head: _, tail } => match *tail {
                Block::Close { semicolon } => assert!(semicolon),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_storage() {
        let input = "storage { x: BigInt; y: F32; }";
        test_with_diagnostics(input, storage());
    }

    #[test]
    fn parse_abi() {
        let input = "abi Abi { fn foo(): number; fn bar(Value); effect Effect1(Value): number; }";
        test_with_diagnostics(input, abi());
    }

    #[test]
    fn parse_impl() {
        let input = "impl Contract { fn foo(x: Int, y:Int): number { let x = 3; yield 3 } }";
        test_with_diagnostics(input, r#impl());
    }

    #[test]
    fn parse_token() {
        let input = "token Token1 { bind { let mut caller = 3; } unbind { let x = 4 + 5; } }";
        test_with_diagnostics(input, token());
    }

    #[test]
    fn parse_utxo() {
        let input = "utxo Contract { main {} }";
        test_with_diagnostics(input, utxo());
    }

    #[test]
    fn parse_program() {
        let input = "abi Abi {} utxo Contract {  main {} } token Token {}";
        test_with_diagnostics(input, starstream_program());
    }

    #[test]
    fn parse_type_arg() {
        let input = "Intermediate<T, any>";
        test_with_diagnostics(input, type_arg());

        let input = "(x: Int) -> Bool";
        test_with_diagnostics(input, type_arg());

        let input = "(x: Int)";
        test_with_diagnostics(input, type_arg());
    }

    #[test]
    fn parse_type_def() {
        let input = "typedef E = enum { One(), Two(x:Int) }";
        test_with_diagnostics(input, typedef());

        let input = "typedef E = { x: Int, y: String }";
        test_with_diagnostics(input, typedef());

        let input = "typedef A = Intermediate<T, any>";
        test_with_diagnostics(input, typedef());
    }

    #[test]
    fn parse_usdc_example() {
        let input = include_str!("../../../grammar/examples/permissioned_usdc.star");
        test_with_diagnostics(input, starstream_program());
    }

    #[test]
    fn parse_oracle_example() {
        let input = include_str!("../../../grammar/examples/oracle.star");
        test_with_diagnostics(input, starstream_program());
    }

    #[test]
    fn parse_fn_with_effects() {
        let input = "script { fn test() / { SomeEffect } {} }";
        test_with_diagnostics(input, script());

        let input = "script { fn test(): u32 / { SomeEffect } {} }";
        test_with_diagnostics(input, script());

        let input = "script { fn test(): u32 {} }";
        test_with_diagnostics(input, script());
    }
}
