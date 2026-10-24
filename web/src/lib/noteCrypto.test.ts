import { describe, expect, it } from "vitest";

import { cryptoVectors } from "./cryptoVectors";
import {
  ckAad,
  fromBase64,
  fromHex,
  fromRecoveryCode,
  noteAad,
  openNote,
  recoveryCode,
  sealNote,
  toBase64,
  toHex,
} from "./noteCrypto";

const note = cryptoVectors.note;
const mk = fromHex(note.mk_hex);
const ck = fromHex(note.ck_hex);

describe("AAD 构造", () => {
  it("与 Rust aad() 逐字节一致", () => {
    expect(toHex(ckAad(note.note_id))).toBe(note.aad_ck_hex);
    expect(toHex(noteAad(note.note_id, note.rev))).toBe(note.aad_note_hex);
  });
});

describe("sealNote", () => {
  it("固定 nonce 时密文与 Rust 完全一致", () => {
    const sealed = sealNote(mk, note.note_id, note.rev, ck, note.plaintext, {
      wrapNonce: fromHex(note.wrap.nonce_hex),
      bodyNonce: fromHex(note.body.nonce_hex),
    });
    expect(sealed.v).toBe(note.v);
    expect(sealed.rev).toBe(note.rev);
    expect(toHex(fromBase64(sealed.wrap.ct))).toBe(note.wrap.ct_hex);
    expect(toHex(fromBase64(sealed.body.ct))).toBe(note.body.ct_hex);
  });

  it("随机 nonce 也能自洽解开", () => {
    const sealed = sealNote(mk, "note-random", 1, ck, {
      title: "标题",
      markdown: "正文",
    });
    expect(sealed.wrap.nonce).not.toBe(sealed.body.nonce);
    expect(openNote(mk, "note-random", sealed)).toEqual({
      title: "标题",
      markdown: "正文",
    });
  });
});

describe("openNote", () => {
  it("能解开 Rust 生成的密文", () => {
    const fromRust = {
      v: note.v,
      rev: note.rev,
      wrap: {
        nonce: toBase64(fromHex(note.wrap.nonce_hex)),
        ct: toBase64(fromHex(note.wrap.ct_hex)),
      },
      body: {
        nonce: toBase64(fromHex(note.body.nonce_hex)),
        ct: toBase64(fromHex(note.body.ct_hex)),
      },
    };
    expect(openNote(mk, note.note_id, fromRust)).toEqual(note.plaintext);
  });

  it("密文被篡改时抛错", () => {
    const sealed = sealNote(mk, "note-tamper", 1, ck, { title: "a", markdown: "b" });
    const tamperedBytes = fromBase64(sealed.body.ct);
    tamperedBytes[0] = (tamperedBytes[0] ?? 0) ^ 0xff;
    const tampered = {
      ...sealed,
      body: { ...sealed.body, ct: toBase64(tamperedBytes) },
    };
    expect(() => openNote(mk, "note-tamper", tampered)).toThrow();
  });

  it("rev 不匹配时抛错（AAD 绑定）", () => {
    const sealed = sealNote(mk, "note-rev", 2, ck, { title: "a", markdown: "b" });
    expect(() => openNote(mk, "note-rev", { ...sealed, rev: 3 })).toThrow();
  });

  it("拒绝未知版本", () => {
    const sealed = sealNote(mk, "note-v", 1, ck, { title: "a", markdown: "b" });
    expect(() => openNote(mk, "note-v", { ...sealed, v: 2 })).toThrow(/版本/);
  });
});

describe("恢复码", () => {
  it("往返一致且是 base64url（无填充）", () => {
    const code = recoveryCode(mk);
    expect(code).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(toHex(fromRecoveryCode(code))).toBe(note.mk_hex);
  });
});
