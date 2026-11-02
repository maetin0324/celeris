//! module loader（境界 ADR 2026-11-02 (c)）: bundle 自身と `celeris:host` 以外の import を拒否する。
//!
//! bundle は依存を同梱した単一の ESM なので、解決してよい specifier は 2 つだけ。相対 path・`file:`・`http(s):`・
//! `node:`・`npm:`・`ext:`・`data:`・動的 import の別 module はすべて resolve の段で落とす。

use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};
use deno_error::JsErrorBox;

use crate::dto::HOST_MODULE;

/// `celeris:host` の本文（静的。op の結線と `HostError` だけで、業務の処理を持たない）。
const HOST_JS: &str = include_str!("host.js");

pub(crate) struct BundleLoader {
    bundle: ModuleSpecifier,
    code: String,
    host: ModuleSpecifier,
}

impl BundleLoader {
    pub(crate) fn new(bundle: ModuleSpecifier, code: String) -> Result<Self, JsErrorBox> {
        let host = ModuleSpecifier::parse(HOST_MODULE).map_err(JsErrorBox::from_err)?;
        Ok(Self { bundle, code, host })
    }
}

/// 拒否の文言。試験と運用ログで探せるよう固定の接頭辞を付ける。
pub(crate) const IMPORT_REJECTED: &str = "celeris: import rejected";

impl ModuleLoader for BundleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        if specifier == self.host.as_str() {
            return Ok(self.host.clone());
        }
        if specifier == self.bundle.as_str() {
            return Ok(self.bundle.clone());
        }
        Err(JsErrorBox::type_error(format!(
            "{IMPORT_REJECTED}: {specifier:?} from {referrer:?} (only the bundle itself and {HOST_MODULE} may be imported)"
        )))
    }

    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let code = if *specifier == self.host {
            HOST_JS.to_string()
        } else if *specifier == self.bundle {
            self.code.clone()
        } else {
            return ModuleLoadResponse::Sync(Err(JsErrorBox::type_error(format!(
                "{IMPORT_REJECTED}: {specifier}"
            ))));
        };
        ModuleLoadResponse::Sync(Ok(ModuleSource::new(
            ModuleType::JavaScript,
            ModuleSourceCode::String(code.into()),
            specifier,
            None,
        )))
    }
}
