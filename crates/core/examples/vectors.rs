//! 生成跨语言测试向量（KAT）：Rust 是权威实现，JS 客户端必须逐字节一致。
//!
//! 用法：`cargo run -p cista-core --example vectors > web/src/lib/cryptoVectors.json`
//! 或直接看 stdout 粘贴到 `web/src/lib/cryptoVectors.ts`。
//!
//! 所有随机量都固定（nonce/种子），输出可复现；不要把这些密钥用于真实数据。

use cista_core::auth::{DeviceSecret, RequestMeta};
use cista_core::crypto::{
    ck_aad, note_aad, seal_with_nonce, Key, NotePlaintext, NONCE_LEN, NOTE_VERSION,
};
use serde_json::json;

fn hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn main() {
    // 1) 笔记信封：MK 包裹 CK，CK 加密正文。
    let mk = Key::from_bytes([0x01; 32]);
    let ck = Key::from_bytes([0x02; 32]);
    let note_id = "note-vec-1";
    let rev: u64 = 7;
    let plaintext = NotePlaintext {
        title: "向量标题".to_string(),
        markdown: "# 内容\n服务端不可读".to_string(),
        attachments: Vec::new(),
    };
    let plaintext_json = serde_json::to_vec(&plaintext).expect("json");

    let wrap_nonce = [0x11u8; NONCE_LEN];
    let body_nonce = [0x22u8; NONCE_LEN];
    let aad_ck = ck_aad(note_id);
    let aad_note = note_aad(note_id, rev);
    let wrap = seal_with_nonce(&mk, &aad_ck, ck.as_bytes(), &wrap_nonce).expect("wrap");
    let body = seal_with_nonce(&ck, &aad_note, &plaintext_json, &body_nonce).expect("body");

    // 2) B+ 请求签名：固定设备种子、nonce、时间戳。
    let seed = [0x07u8; 32];
    let device = DeviceSecret::from_seed(&seed);
    let req_method = "PUT";
    let req_path = "/api/notes/note-vec-1";
    let req_body = br#"{"sealed":{"v":1}}"#;
    let req_nonce = [0x33u8; 16];
    let req_ts: i64 = 1_700_000_000;
    let meta = RequestMeta::new(req_method, req_path, req_body, &req_nonce, req_ts);
    let signature = device.sign(&meta);
    let canonical = String::from_utf8(meta.canonical()).expect("utf8");

    let out = json!({
        "note": {
            "mk_hex": hex(mk.as_bytes()),
            "ck_hex": hex(ck.as_bytes()),
            "note_id": note_id,
            "rev": rev,
            "v": NOTE_VERSION,
            "plaintext": { "title": plaintext.title, "markdown": plaintext.markdown },
            "plaintext_json_hex": hex(&plaintext_json),
            "aad_ck_hex": hex(&aad_ck),
            "aad_note_hex": hex(&aad_note),
            "wrap": { "nonce_hex": hex(&wrap.nonce), "ct_hex": hex(&wrap.ct) },
            "body": { "nonce_hex": hex(&body.nonce), "ct_hex": hex(&body.ct) }
        },
        "auth": {
            "seed_hex": hex(&seed),
            "pubkey_hex": hex(&device.public_key()),
            "device_id": device.device_id(),
            "method": req_method,
            "path": req_path,
            "body_hex": hex(req_body),
            "body_sha256_hex": hex(&meta.body_sha256),
            "nonce_hex": hex(&req_nonce),
            "ts": req_ts,
            "canonical": canonical,
            "signature_hex": signature
        }
    });
    println!("{}", serde_json::to_string_pretty(&out).expect("json out"));
}
