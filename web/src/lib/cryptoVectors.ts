/**
 * 跨语言测试向量（KAT）：由 Rust 权威实现生成。
 *
 * 生成命令：`cargo run -p cista-core --example vectors`
 * 输出逐字段复制到这里；任何一方改动加密/签名格式都必须同时更新向量与两端测试。
 *
 * 固定密钥仅用于测试，禁止用于真实数据。
 */
export const cryptoVectors = {
  note: {
    mk_hex: "0101010101010101010101010101010101010101010101010101010101010101",
    ck_hex: "0202020202020202020202020202020202020202020202020202020202020202",
    note_id: "note-vec-1",
    rev: 7,
    v: 1,
    plaintext: { title: "向量标题", markdown: "# 内容\n服务端不可读" },
    plaintext_json_hex:
      "7b227469746c65223a22e59091e9878fe6a087e9a298222c226d61726b646f776e223a222320e58685e5aeb95c6ee69c8de58aa1e7abafe4b88de58fafe8afbb227d",
    aad_ck_hex: "63697374612e636b2e7631000000000a6e6f74652d7665632d31",
    aad_note_hex:
      "63697374612e6e6f74652e7631000000000a6e6f74652d7665632d31000000080000000000000007",
    wrap: {
      nonce_hex: "111111111111111111111111111111111111111111111111",
      ct_hex:
        "38bcee717f61b2af5bd0ae07c3f067e3dce866b93ada9587721fdaf4aea63e269dc3ddd6a9157961eb753f28e65a4f7d",
    },
    body: {
      nonce_hex: "222222222222222222222222222222222222222222222222",
      ct_hex:
        "af2a043db7ac73b4bcc7081fbcba3a43c39f10d82755577e0e003c5a98d7ea05dfb13a1b2456d24a371c857b75904de828fd9374ecdd8be69fa326e2dea50ad4df49954e5c237952862e2fb1a6b5a8d45937",
    },
  },
  auth: {
    seed_hex: "0707070707070707070707070707070707070707070707070707070707070707",
    pubkey_hex: "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c",
    device_id: "fe812c12f3ab4ce6ac5db69ac352f906",
    method: "PUT",
    path: "/api/notes/note-vec-1",
    body_hex: "7b227365616c6564223a7b2276223a317d7d",
    body_sha256_hex: "d0bab976495471cc8129f552e1faba9ff7a7db3a78abe6ccca4a53fe55c73dc7",
    nonce_hex: "33333333333333333333333333333333",
    ts: 1_700_000_000,
    canonical:
      "cista.b+1\nPUT\n/api/notes/note-vec-1\nd0bab976495471cc8129f552e1faba9ff7a7db3a78abe6ccca4a53fe55c73dc7\n33333333333333333333333333333333\n1700000000",
    signature_hex:
      "44b81be176079e3aeae5178e2eeb43b9288dc97cc83374dcbe96c464055956f881e25d0b8bbb6838c87c3168d6bb854095f55ab31ad12e158df7cf8b7810ed0e",
  },
} as const;
