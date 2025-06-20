//! AST types describing a Starstream source file.

/// The root type of a Starstream source file.
#[derive(Clone, Debug, Default)]
pub struct StarstreamProgram {
    pub items: Vec<ProgramItem>,
}

/// A coordination script, UTXO, or token definition block.
#[derive(Clone, Debug)]
pub enum ProgramItem {
    // TODO: Import
    Script(Script),
    Utxo(Utxo),
    Token(Token),
}

/// `utxo Name { ... }`
#[derive(Clone, Debug)]
pub struct Utxo {
    pub name: Identifier,
    pub items: Vec<UtxoItem>,
}

#[derive(Clone, Debug)]
pub enum UtxoItem {
    Abi(Abi),
    Main(Main),
    Impl(Impl),
    Storage(Storage),
}

#[derive(Clone, Debug)]
pub struct Main {
    pub type_sig: Option<OptionallyTypedBindings>,
    pub block: Block,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub name: Identifier,
    pub items: Vec<TokenItem>,
}

#[derive(Clone, Debug)]
pub enum TokenItem {
    Bind(Bind),
    Unbind(Unbind),
    Abi(Abi),
    Mint(Mint),
}

#[derive(Clone, Debug)]
pub struct Bind(pub Block);

#[derive(Clone, Debug)]
pub struct Unbind(pub Block);

#[derive(Clone, Debug)]
pub struct Mint(pub Block);

#[derive(Clone, Debug)]
pub struct Impl {
    pub name: Identifier,
    pub definitions: Vec<FnDef>,
}

#[derive(Clone, Debug)]
pub struct Script {
    pub definitions: Vec<FnDef>,
}

#[derive(Clone, Debug)]
pub struct Storage {
    pub bindings: TypedBindings,
}

#[derive(Clone, Debug)]
pub struct Sig {
    pub name: Identifier,
    pub input_types: Vec<Type>,
    pub output_type: Option<Type>,
}

#[derive(Clone, Debug)]
pub struct FnSig(pub Sig);

#[derive(Clone, Debug)]
pub struct FnDef {
    pub name: Identifier,
    pub inputs: OptionallyTypedBindings,
    pub output: Option<Type>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub enum EffectSig {
    EffectSig(Sig),
    EventSig(Sig),
    ErrorSig(Sig),
}

#[derive(Clone, Debug)]
pub enum AbiElem {
    FnSig(FnSig),
    EffectSig(EffectSig),
}

#[derive(Clone, Debug)]
pub struct Abi {
    pub values: Vec<AbiElem>,
}

#[derive(Clone, Debug)]
pub struct Identifier(pub String);

#[derive(Clone, Debug)]
pub enum Type {
    BaseType(Identifier, Option<Vec<Type>>),
    Object(TypedBindings),
    FnType(TypedBindings, Option<Box<Type>>),
}

#[derive(Clone, Debug)]
pub enum Statement {
    /// `let [mut] a = b;`
    BindVar {
        var: Identifier,
        mutable: bool,
        value: Expr,
    },
    /// `return a;`
    Return(Option<Expr>),
    /// `resume a;`
    Resume(Option<Expr>),
    /// `a = b;`
    Assign(Identifier, Expr),
    /// `with { a... } catch (b) { c... } ...`
    With(Block, Vec<(Effect, Block)>),
    /// `while (a) { b... }`
    While(Expr, LoopBody),
    /// `loop { a... }`
    Loop(LoopBody),
}

#[derive(Clone, Debug)]
pub enum LoopBody {
    Statement(Box<Statement>),
    Block(Block),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub enum Expr {
    PrimaryExpr(
        /// Starter expression.
        PrimaryExpr,
        /// If followed by a function call `(args...)`.
        Option<Arguments>,
        /// Following fields `.ident` or method calls `.ident(args...)`.
        Vec<(Identifier, Option<Arguments>)>,
    ),
    BlockExpr(BlockExpr),
    // Comparison operators
    /// `a == b`
    Equals(Box<Self>, Box<Self>),
    /// `a != b`
    NotEquals(Box<Self>, Box<Self>),
    /// `a < b`
    LessThan(Box<Self>, Box<Self>),
    /// `a > b`
    GreaterThan(Box<Self>, Box<Self>),
    /// `a <= b`
    LessEq(Box<Self>, Box<Self>),
    /// `a >= b`
    GreaterEq(Box<Self>, Box<Self>),
    // Arithmetic operators
    /// `a + b`
    Add(Box<Self>, Box<Self>),
    /// `a - b`
    Sub(Box<Self>, Box<Self>),
    /// `a * b`
    Mul(Box<Self>, Box<Self>),
    /// `a / b`
    Div(Box<Self>, Box<Self>),
    /// `a % b`
    Mod(Box<Self>, Box<Self>),
    /// `-a`
    Neg(Box<Self>),
    // Bitwise operators
    /// `~a`
    BitNot(Box<Self>),
    /// `a & b`
    BitAnd(Box<Self>, Box<Self>),
    /// `a | b`
    BitOr(Box<Self>, Box<Self>),
    /// `a ^ b`
    BitXor(Box<Self>, Box<Self>),
    /// `a << b`
    LShift(Box<Self>, Box<Expr>),
    /// `a >> b`
    RShift(Box<Self>, Box<Expr>),
    // Boolean operators
    /// `!a`
    Not(Box<Self>),
    /// `a && b`
    And(Box<Self>, Box<Self>),
    /// `a || b`
    Or(Box<Self>, Box<Self>),
}

#[derive(Clone, Debug)]
pub enum BlockExpr {
    /// `if (a) { b } else { c }`
    IfThenElse(Box<Expr>, Box<Block>, Option<Box<Block>>),
    /// `{ a... }`
    Block(Block),
}

#[derive(Clone, Debug)]
pub enum PrimaryExpr {
    Null,
    Number(f64),
    /// `true` or `false` literal
    Bool(bool),
    Ident(Vec<Identifier>),
    /// `(a)`
    ParExpr(Box<Expr>),
    /// `yield a`
    Yield(Box<Expr>),
    /// `raise a`
    Raise(Box<Expr>),
    /// `a { b: c, ... }`
    Object(Type, Vec<(Identifier, Expr)>),
    StringLiteral(String),
}

#[derive(Clone, Debug)]
pub enum ExprOrStatement {
    Expr(Expr),
    Statement(Statement),
}

#[derive(Clone, Debug)]
pub enum Block {
    Chain {
        head: Box<ExprOrStatement>,
        tail: Box<Block>,
    },
    Close {
        semicolon: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Arguments {
    pub xs: Vec<Expr>,
}

#[derive(Clone, Debug)]
pub struct OptionallyTypedBindings {
    pub values: Vec<(Identifier, Option<Type>)>,
}

#[derive(Clone, Debug)]
pub struct TypedBindings {
    pub values: Vec<(Identifier, Type)>,
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub ident: Identifier,
    pub type_sig: OptionallyTypedBindings,
}
