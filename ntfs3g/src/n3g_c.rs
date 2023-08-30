#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(clippy::upper_case_acronyms)]
#![allow(clippy::useless_transmute)]
#![allow(dead_code)]
// XXX remove me when bindgen bump version
#![allow(clippy::incorrect_clone_impl_on_copy_type)]
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
