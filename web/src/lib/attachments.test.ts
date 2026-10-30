import { afterEach, describe, expect, it, vi } from "vitest";

import {
  downloadAttachment,
  prepareAttachment,
  uploadAttachment,
} from "./attachments";
import { deviceFromSeed } from "./authClient";
import { cryptoVectors } from "./cryptoVectors";
import { generateKey, fromHex } from "./noteCrypto";
import type { Session } from "./notesApi";

const session: Session = {
  apiBase: "http://api.test",
  device: deviceFromSeed(fromHex(cryptoVectors.auth.seed_hex)),
  mk: fromHex(cryptoVectors.note.mk_hex),
};

const source = (text: string, name = "报告.txt") => ({
  name,
  async arrayBuffer() {
    return new TextEncoder().encode(text).buffer;
  },
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("prepareAttachment", () => {
  it("上传字节是密文、哈希按上传字节算、大小是明文长度", async () => {
    const prepared = await prepareAttachment(session, source("hello attachment"));
    expect(prepared.bytes.length).toBe(24 + "hello attachment".length + 16);
    expect(prepared.attachment.name).toBe("报告.txt");
    expect(prepared.attachment.size).toBe(16);
    expect(prepared.attachment.hash).toMatch(/^[0-9a-f]{64}$/);
    expect(
      new TextDecoder().decode(prepared.bytes).includes("hello attachment"),
    ).toBe(false);
  });
});

describe("upload / download", () => {
  it("上传带签名与幂等键，取回后可解出原文", async () => {
    const prepared = await prepareAttachment(session, source("roundtrip"));
    const calls: { url: string; init: RequestInit }[] = [];
    vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
      const call = { url: String(input), init: init ?? {} };
      calls.push(call);
      if (init?.method === "POST") return new Response("{}", { status: 201 });
      return new Response(prepared.bytes, { status: 200 });
    });

    await uploadAttachment(session, prepared);
    const upload = calls[0]!;
    expect(upload.url).toBe("http://api.test/api/files");
    expect((upload.init.headers as Record<string, string>)["idempotency-key"]).toBe(
      prepared.attachment.hash,
    );
    expect((upload.init.headers as Record<string, string>)["x-signature"]).toMatch(
      /^[0-9a-f]{128}$/,
    );
    expect(upload.init.body).toEqual(prepared.bytes);

    const restored = await downloadAttachment(session, prepared.attachment);
    expect(new TextDecoder().decode(restored)).toBe("roundtrip");
  });

  it("用错 MK 无法解出附件", async () => {
    const prepared = await prepareAttachment(session, source("secret"));
    vi.stubGlobal("fetch", async () => new Response(prepared.bytes, { status: 200 }));
    const wrong = { ...session, mk: generateKey() };
    await expect(downloadAttachment(wrong, prepared.attachment)).rejects.toThrow();
  });

  it("下载失败时抛出状态码", async () => {
    const prepared = await prepareAttachment(session, source("x"));
    vi.stubGlobal("fetch", async () => new Response("nope", { status: 404 }));
    await expect(downloadAttachment(session, prepared.attachment)).rejects.toThrow(
      /404/,
    );
  });
});
