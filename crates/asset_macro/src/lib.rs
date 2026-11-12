// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `crates/asset_macro/src/lib.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! Macros that reference files in the repository's `assets/` directory.
//!
//! - `bundled_asset!("path")`: a file under `assets/bundled`, embedded in the binary.
//! - `bundled_or_fetched_asset!("path")`: a file under `assets/async` (large, optional assets
//!   such as theme backgrounds). Warp fetched these remotely on the web; MatterLite always
//!   bundles them.
//!
//! The macros check at compile time that the file exists and expand to an `AssetSource`.
//! A second argument selects another folder under `assets/`.

#![recursion_limit = "1024"]
#[macro_use]
extern crate quote;
extern crate proc_macro;

use std::path::PathBuf;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use syn::parse::Parse;
use syn::{LitStr, Token, parse_macro_input};

const BUNDLED_ASSETS_DIR: &str = "bundled";
const ASYNC_ASSETS_DIR: &str = "async";

struct MacroArgs {
    /// The name of the asset. E.g. `jpg/jellyfish_bg.jpg`
    asset_name: LitStr,
    /// The asset subfolder under `assets`. E.g. `async`.
    asset_folder: Option<LitStr>,
}

impl Parse for MacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        // Parse either one string literal (the asset location) or two comma-separated string
        // literals (the asset location and the asset subfolder).
        Ok(MacroArgs {
            asset_name: input.parse()?,
            asset_folder: if input.peek(Token![,]) {
                let _comma: Token![,] = input.parse()?;
                Some(input.parse()?)
            } else {
                None
            },
        })
    }
}

#[proc_macro]
pub fn bundled_asset(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as MacroArgs);
    let asset_name = args.asset_name.value();
    let asset_folder_arg = args.asset_folder.map(|s| s.value());
    let asset_folder = asset_folder_arg.as_deref().unwrap_or(BUNDLED_ASSETS_DIR);

    match construct_bundled_asset(&asset_name, asset_folder) {
        Ok(ok) => ok.into(),
        Err(err_str) => format_error(&asset_name, asset_folder, err_str).into(),
    }
}

fn construct_bundled_asset(asset_name: &str, asset_dir: &str) -> Result<TokenStream2, String> {
    if full_asset_path(asset_name, asset_dir).exists() {
        let full_location = format!("{asset_dir}/{asset_name}");
        Ok(quote! {
            ::warpui_core::assets::asset_cache::AssetSource::Bundled {
                path: #full_location .into(),
            }
        })
    } else {
        Err("file not found".into())
    }
}

#[proc_macro]
pub fn bundled_or_fetched_asset(input: TokenStream) -> TokenStream {
    let input_lit = parse_macro_input!(input as LitStr);
    quote! {
        ::asset_macro::bundled_asset!( #input_lit, #ASYNC_ASSETS_DIR )
    }
    .into()
}

fn full_asset_path(asset_name: &str, asset_dir: &str) -> PathBuf {
    // All crates share the repository's `assets/` directory, two levels above this crate.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(asset_dir)
        .join(asset_name)
}

fn format_error(asset_name: &str, asset_dir: &str, error_string: String) -> TokenStream2 {
    let full_path = full_asset_path(asset_name, asset_dir);
    let error_message = format!("Error loading asset at {full_path:?}: {error_string}");

    quote! {
        compile_error!(#error_message)
    }
}
