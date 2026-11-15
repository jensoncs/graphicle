//! A Spring controller calls `productService.getRechargeOptions(...)` through
//! its interface; the implementation is reachable only through a dispatch
//! edge from the interface's method (precise mode, Java and Kotlin).
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;

fn dispatches(files: &[(&str, &str)]) -> Vec<(String, String)> {
    graphicle::extract::PRECISE.store(true, std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let paths: Vec<_> = files
        .iter()
        .map(|(name, text)| {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect();
    let options = ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.clone()),
        parallel: false,
        ..Default::default()
    };
    let graph: Value = extract(&paths, &options).unwrap();
    let mut out: Vec<_> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "dispatches_to")
        .map(|e| {
            (
                e["source"].as_str().unwrap().to_string(),
                e["target"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn java_interface_methods_dispatch_to_their_implementation() {
    let got = dispatches(&[
        (
            "svc/IProductService.java",
            "package svc;\npublic interface IProductService {\n    String getRechargeOptions(String userId);\n}\n",
        ),
        (
            "svc/impl/ProductService.java",
            "package svc.impl;\nimport svc.IProductService;\npublic class ProductService implements IProductService {\n\
             @Override\n    public String getRechargeOptions(String userId) { return userId; }\n}\n",
        ),
    ]);
    assert_eq!(
        got,
        [(
            "svc_iproductservice_iproductservice_getrechargeoptions".to_string(),
            "svc_impl_productservice_productservice_getrechargeoptions".to_string()
        )],
    );
}

#[test]
fn kotlin_interface_methods_dispatch_to_their_implementation() {
    let got = dispatches(&[
        (
            "svc/Wallet.kt",
            "package svc\ninterface Wallet {\n    fun debit(amount: Long): Boolean\n}\n",
        ),
        (
            "svc/SpannerWallet.kt",
            "package svc\nclass SpannerWallet : Wallet {\n    override fun debit(amount: Long): Boolean = amount > 0\n}\n",
        ),
    ]);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].0.ends_with("wallet_debit") && got[0].1.ends_with("spannerwallet_debit"),
        "{got:?}"
    );
}
