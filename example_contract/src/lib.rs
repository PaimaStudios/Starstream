#![feature(extern_types)]
#![no_std]

use starstream::{Utxo, UtxoCoroutine};

// "starstream:example_contract" should probably be something content-addressed
#[link(wasm_import_module = "starstream:example_contract")]
unsafe extern "C" {
    pub type MyMain;
    safe fn starstream_status_MyMain(utxo: Utxo<MyMain>) -> bool;
    unsafe fn starstream_resume_MyMain(
        utxo: Utxo<MyMain>,
        resume_arg: *const (),
        resume_arg_size: usize,
    );
    safe fn starstream_new_MyMain_new() -> Utxo<MyMain>;
    safe fn starstream_query_MyMain_get_supply(utxo: Utxo<MyMain>) -> u32;
}

impl UtxoCoroutine for MyMain {
    type Resume = ();

    #[inline]
    fn ffi_status(utxo: Utxo<Self>) -> bool {
        starstream_status_MyMain(utxo)
    }

    #[inline]
    fn ffi_resume(utxo: Utxo<Self>, arg: ()) {
        unsafe {
            starstream_resume_MyMain(
                utxo,
                &raw const arg as *const (),
                core::mem::size_of::<Self::Resume>(),
            )
        }
    }
}

impl MyMain {
    #[inline]
    pub fn new() -> Utxo<MyMain> {
        starstream_new_MyMain_new()
    }
}

pub trait MyMainExt {
    fn get_supply(self) -> u32;
}

impl MyMainExt for Utxo<MyMain> {
    #[inline]
    fn get_supply(self) -> u32 {
        starstream_query_MyMain_get_supply(self)
    }
}
