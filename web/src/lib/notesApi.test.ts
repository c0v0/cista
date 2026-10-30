import { afterEach, describe, expect, it, vi } from "vitest";

import { deviceFromSeed } from "./authClient";
import { cryptoVectors } from "./cryptoVectors";
import { fromHex, generateKey, sealNote, toHex } from "./noteCrypto";
import {
  ApiError,
  decryptNote,
  enrollDevice,
  listNotes,
  newNoteId,
  saveNote,
  type Session,
} from "./notesApi";

const device = deviceFromSeed(fromHex(cryptoVectors.auth.seed_hex));
const session: Session = {
  apiBase: "http://api.test",
  device,
  mk: fromHex(cryptoVectors.note.mk_hex),
};

interface Call {
  url: string;
  init: RequestInit;
}

function stubFetch(respond: (call: Call) => Response): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    const call: Call = { url: String(input), init: init ?? {} };
    calls.push(call);
    return respond(call);
  });
  return calls;
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function headersOf(call: Call): Record<string, string> {
  return (call.init.headers ?? {}) as Record<string, string>;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("enrollDevice", () => {
  it("用 Bootstrap 头入网并校验 device_id", async () => {
    const calls = stubFetch(() => jsonResponse(201, { device_id: device.id }));
    const enrolled = await enrollDevice(
      "http://api.test",
      "bootstrap-1",
      "我的桌面",
      fromHex(cryptoVectors.auth.seed_hex),
    );
    expect(enrolled.id).toBe(cryptoVectors.auth.device_id);
    expect(calls[0]?.url).toBe("http://api.test/api/devices/enroll");
    expect(headersOf(calls[0]!).authorization).toBe("Bootstrap bootstrap-1");
    expect(JSON.parse(String(calls[0]!.init.body))).toEqual({
      name: "我的桌面",
      pubkey: cryptoVectors.auth.pubkey_hex,
    });
  });

  it("服务端返回的 device_id 不一致时失败", async () => {
    stubFetch(() => jsonResponse(201, { device_id: "deadbeef" }));
    await expect(
      enrollDevice(
        "http://api.test",
        "bootstrap-1",
        "dev",
        fromHex(cryptoVectors.auth.seed_hex),
      ),
    ).rejects.toThrow(/device_id/);
  });
});

describe("signed requests", () => {
  it("listNotes 带齐 B+ 头且路径带 since", async () => {
    const calls = stubFetch(() => jsonResponse(200, { notes: [], next_since: 0 }));
    await listNotes(session, 5);
    const call = calls[0]!;
    expect(call.url).toBe("http://api.test/api/notes?since=5");
    const headers = headersOf(call);
    expect(headers["x-device-id"]).toBe(device.id);
    expect(headers["x-signature"]).toMatch(/^[0-9a-f]{128}$/);
    expect(headers["x-nonce"]).toMatch(/^[0-9a-f]{32}$/);
  });

  it("saveNote 带 If-Match 且 sealed.rev 与目标 rev 一致", async () => {
    const calls = stubFetch(() => jsonResponse(200, { id: "n1", rev: 2 }));
    await saveNote(session, "n1", 2, { title: "t", markdown: "m" }, 1);
    const call = calls[0]!;
    expect(call.url).toBe("http://api.test/api/notes/n1");
    expect(call.init.method).toBe("PUT");
    expect(headersOf(call)["if-match"]).toBe("1");
    const body = JSON.parse(new TextDecoder().decode(call.init.body as Uint8Array)) as {
      sealed: { rev: number; v: number };
    };
    expect(body.sealed.rev).toBe(2);
    expect(body.sealed.v).toBe(1);
  });

  it("409 映射成 rev 冲突", async () => {
    stubFetch(() => jsonResponse(409, { error: "rev_conflict", rev: 9 }));
    const failure = await saveNote(session, "n1", 2, { title: "t", markdown: "m" }, 1).catch(
      (error: unknown) => error,
    );
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).isRevConflict).toBe(true);
    expect((failure as ApiError).rev).toBe(9);
  });
});

describe("decryptNote", () => {
  it("能解出客户端自己封装的记录", () => {
    const sealed = sealNote(session.mk, "n1", 3, generateKey(), {
      title: "标题",
      markdown: "正文",
    });
    expect(
      decryptNote(session, { id: "n1", rev: 3, updated_at: 0, sealed }),
    ).toEqual({ title: "标题", markdown: "正文" });
  });
});

describe("newNoteId", () => {
  it("32 位 hex 且每次不同", () => {
    const first = newNoteId();
    expect(first).toMatch(/^[0-9a-f]{32}$/);
    expect(newNoteId()).not.toBe(first);
    expect(toHex(fromHex(first))).toBe(first);
  });
});
