#![no_std]
#![no_main]
#![allow(dead_code)]

use example_contract_permissioned::{PermissionedToken, TokenIntermediate};
use starstream::{
    Effect, EffectHandler, Token, TokenStorage, Utxo, eprintln, run_effectful_computation,
    token_export,
};

starstream::panic_handler!();

const PERMISSIONED_TOKEN_ID: u64 = 1003;

pub struct PayToPublicKeyHash {
    owner: i32,
    token: Option<PermissionedToken>,
}

impl PayToPublicKeyHash {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(owner: i32, sleep: fn(&mut Self)) {
        // It's currently the TX where the UTXO is created.
        let mut this = PayToPublicKeyHash { owner, token: None };

        sleep(&mut this);

        if let Some(token) = this.token {
            let intermediate = token.unbind();

            // TODO: maybe the unbind should do this by default?
            TokenUnbound::raise(&intermediate);
        }

        // TODO: assert signature so that only the owner can consume this
    }

    pub fn get_owner(&self) -> i32 {
        self.owner
    }

    // TODO: generalize
    pub fn attach(&mut self, i: TokenIntermediate) {
        self.token = Some(PermissionedToken::bind(i));
    }

    pub fn burn(self) {}
}

pub struct LinkedListNode {
    key: i32,
    next: i32,
}

impl LinkedListNode {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(key: i32, next: i32, sleep: fn(&mut Self)) {
        let mut this = LinkedListNode { key, next };
        sleep(&mut this);
    }

    pub fn get_key(&self) -> i32 {
        self.key
    }

    pub fn get_next(&self) -> i32 {
        self.next
    }

    pub fn burn(self) {}

    // pub fn attach<T: Token>(&mut self, i: T::Intermediate) {
    //     T::bind(i);
    // }
}

// ----------------------------------------------------------------------------
// Generated

#[unsafe(no_mangle)]
pub extern "C" fn starstream_new_PayToPublicKeyHash_new(owner: i32) {
    PayToPublicKeyHash::new(owner, starstream::sleep_mut::<(), PayToPublicKeyHash>)
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_query_PayToPublicKeyHash_get_owner(this: &PayToPublicKeyHash) -> i32 {
    this.get_owner()
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_mutate_PayToPublicKeyHash_attach(
    this: &mut PayToPublicKeyHash,
    i: TokenIntermediate,
) {
    this.attach(i)
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_new_LinkedListNode_new(key: i32, next: i32) {
    LinkedListNode::new(key, next, starstream::sleep_mut::<(), LinkedListNode>)
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_query_LinkedListNode_get_next(this: &LinkedListNode) -> i32 {
    this.next
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_query_LinkedListNode_get_key(this: &LinkedListNode) -> i32 {
    this.key
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn starstream_consume_LinkedListNode_burn(this: *mut LinkedListNode) {
    unsafe { core::ptr::read(this) }.burn()
}

#[unsafe(no_mangle)]
#[allow(clippy::missing_safety_doc)]
pub unsafe extern "C" fn starstream_consume_PayToPublicKeyHash_burn(this: *mut PayToPublicKeyHash) {
    unsafe { core::ptr::read(this) }.burn()
}

// ----------------------------------------------------------------------------
// Coordination script

#[unsafe(no_mangle)]
pub extern "C" fn transfer_usdc(
    source: example_contract_permissioned::PayToPublicKeyHash,
    proof_from: example_contract_permissioned::LinkedListNode,
    proof_to: example_contract_permissioned::LinkedListNode,
    to: i32,
    to_amount: i32,
) -> example_contract_permissioned::PayToPublicKeyHash {
    let from = source.get_owner();

    let input_amount = core::cell::RefCell::new(0);

    run_effectful_computation(
        EffectHandler::<TokenUnbound>::with(&|token| *input_amount.borrow_mut() += token.amount),
        || {
            // TODO: this should probably yield the tokens, but currently it's not easy
            // to yield something that it's not the utxo handler, so we use an effect
            // instead.
            //
            // although maybe we just need a different function call to get the tokens
            // of a dead utxo? Or maybe unbind should always raise an effect?
            source.next();
        },
    );

    let input_amount = *input_amount.borrow();

    let output_utxo = example_contract_permissioned::PayToPublicKeyHash::new(to);
    let output_amount = to_amount;

    let output_intermediate = example_contract_permissioned::TokenIntermediate {
        amount: output_amount,
    };

    let is_blacklisted_handler = |address| {
        let res1 = is_in_range(proof_from, address);
        let res2 = is_in_range(proof_to, address);

        res1 || res2
    };

    run_effectful_computation(
        (
            EffectHandler::<TxCaller>::with(&|_| from),
            EffectHandler::<IsBlacklisted>::with(&is_blacklisted_handler),
        ),
        || {
            output_utxo.attach(output_intermediate);
        },
    );

    let change_utxo = example_contract_permissioned::PayToPublicKeyHash::new(from);
    let change_intermediate = example_contract_permissioned::TokenIntermediate {
        amount: input_amount
            .checked_sub(output_amount)
            .expect("not enough inputs"),
    };

    run_effectful_computation(
        (
            EffectHandler::<TxCaller>::with(&|_| from),
            EffectHandler::<IsBlacklisted>::with(&is_blacklisted_handler),
        ),
        || {
            change_utxo.attach(change_intermediate);
        },
    );

    output_utxo
}

fn is_in_range(proof: example_contract_permissioned::LinkedListNode, addr: i32) -> bool {
    eprintln!(
        "checking range: {} < {} < {}",
        proof.get_key(),
        addr,
        proof.get_next()
    );

    proof.get_key() < addr && addr < proof.get_next()
}

#[unsafe(no_mangle)]
pub extern "C" fn blacklist_empty() -> example_contract_permissioned::LinkedListNode {
    let key = 0;
    let next = i32::MAX;

    example_contract_permissioned::LinkedListNode::new(key, next)
}

#[unsafe(no_mangle)]
pub extern "C" fn blacklist_insert(
    prev: example_contract_permissioned::LinkedListNode,
    new: i32,
) -> example_contract_permissioned::LinkedListNode {
    let prev_next = prev.get_next();
    let prev_key = prev.get_key();

    prev.burn();

    assert!(prev_key < new);
    assert!(new < prev_next);

    example_contract_permissioned::LinkedListNode::new(prev_key, new);
    example_contract_permissioned::LinkedListNode::new(new, prev_next)
}

#[unsafe(no_mangle)]
pub extern "C" fn blacklist_node_get_key(
    prev: example_contract_permissioned::LinkedListNode,
) -> i32 {
    prev.get_key()
}

#[unsafe(no_mangle)]
pub extern "C" fn token_mint_new() -> example_contract_permissioned::TokenMint {
    example_contract_permissioned::TokenMint::new()
}

#[unsafe(no_mangle)]
pub extern "C" fn token_mint_to(
    minter: example_contract_permissioned::TokenMint,
    owner: i32,
    amount: i32,
    proof: example_contract_permissioned::LinkedListNode,
) -> example_contract_permissioned::PayToPublicKeyHash {
    run_effectful_computation::<_, _>(
        (
            EffectHandler::<IsBlacklisted>::with(&|to_address| is_in_range(proof, to_address)),
            EffectHandler::<TxCaller>::with(&(|_| owner)),
        ),
        || {
            let out = example_contract_permissioned::PayToPublicKeyHash::new(owner);
            let intermediate = minter.mint(amount);
            out.attach(intermediate);

            out
        },
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn pay_to_public_key_hash_owner(
    utxo: example_contract_permissioned::PayToPublicKeyHash,
) -> i32 {
    utxo.get_owner()
}

// Token

pub struct TokenMint {}

impl TokenMint {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(sleep: fn(&mut TokenMint)) {
        let mut this = TokenMint {};
        loop {
            sleep(&mut this);
        }
    }

    pub fn mint(&mut self, amount: i32) -> example_contract_permissioned::TokenIntermediate {
        example_contract_permissioned::TokenIntermediate { amount }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_new_TokenMint_new() {
    TokenMint::new(starstream::sleep_mut::<(), TokenMint>)
}

#[unsafe(no_mangle)]
pub extern "C" fn starstream_mutate_TokenMint_mint(
    this: &mut TokenMint,
    amount: i32,
) -> example_contract_permissioned::TokenIntermediate {
    this.mint(amount)
}

fn starstream_bind_token_inner(this: TokenIntermediate) -> TokenStorage {
    let owner = TxCaller::raise(&());

    let is_blacklisted_output = IsBlacklisted::raise(&owner);

    assert!(is_blacklisted_output);

    TokenStorage {
        id: PERMISSIONED_TOKEN_ID,
        amount: this.amount.try_into().unwrap(),
    }
}
token_export! {
    for TokenIntermediate;
    bind fn starstream_bind_Token(this: Self) -> TokenStorage {
        assert!(starstream::coordination_code() == starstream::this_code());
        starstream_bind_token_inner(this)
    }
    unbind fn starstream_unbind_Token(storage: TokenStorage) -> Self {
        // assert!(starstream::coordination_code() == starstream::this_code());
        TokenIntermediate { amount: storage.amount.try_into().unwrap() }
    }
}

// Effects
//
pub enum IsBlacklisted {}

impl Effect for IsBlacklisted {
    const NAME: &'static str = "IsBlacklisted";

    type Input = i32;
    type Output = bool;
}

#[unsafe(no_mangle)]
pub extern "C" fn IsBlacklisted_handle(this: &EffectHandler<'_, IsBlacklisted>) {
    this.handle();
}

pub enum TxCaller {}

impl Effect for TxCaller {
    const NAME: &'static str = "TxCaller";

    type Input = ();
    type Output = i32;
}

#[unsafe(no_mangle)]
pub extern "C" fn TxCaller_handle(this: &EffectHandler<'_, TxCaller>) {
    this.handle();
}

pub enum TokenUnbound {}

impl Effect for TokenUnbound {
    const NAME: &'static str = "TokenUnbound";

    type Input = TokenIntermediate;
    type Output = ();
}

#[unsafe(no_mangle)]
pub extern "C" fn TokenUnbound_handle(this: &EffectHandler<'_, TokenUnbound>) {
    this.handle();
}
