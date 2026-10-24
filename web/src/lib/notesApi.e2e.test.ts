import { describe, expect, it } from "vitest";

import { generateKey } from "./noteCrypto";
import {
  decryptNote,
  enrollDevice,
  getNote,
  listNotes,
  saveNote,
  type Session,
} from "./notesApi";

/**
 * 对真实服务端的端到端（无浏览器）：配对 → 保存 → 读取解密 → 更新 → 冲突 → 隔离。
 *
 * 需要环境变量（否则整组跳过）：
 *   CISTA_E2E_URL=http://127.0.0.1:18888
 *   CISTA_E2E_BOOTSTRAP=<服务端 CISTA_BOOTSTRAP_TOKEN>
 */
declare const process: { env: Record<string, string | undefined> };

const base = process.env.CISTA_E2E_URL ?? "";
const bootstrap = process.env.CISTA_E2E_BOOTSTRAP ?? "";
const enabled = base.length > 0 && bootstrap.length > 0;

function randomHex(bytes: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

describe.skipIf(!enabled)("notesApi ↔ 真实服务端", () => {
  it("配对、加密保存、解密读取、并发冲突、MK 隔离", async () => {
    const device = await enrollDevice(
      base,
      bootstrap,
      "e2e-web",
      crypto.getRandomValues(new Uint8Array(32)),
    );
    const session: Session = { apiBase: base, device, mk: generateKey() };
    const id = `e2e${randomHex(12)}`;

    const created = await saveNote(session, id, 1, {
      title: "端到端",
      markdown: "服务端不可读",
    });
    expect(created.rev).toBe(1);

    const fetched = await getNote(session, id);
    expect(decryptNote(session, fetched)).toEqual({
      title: "端到端",
      markdown: "服务端不可读",
    });

    const updated = await saveNote(session, id, 2, { title: "v2", markdown: "改过" }, 1);
    expect(updated.rev).toBe(2);

    await expect(
      saveNote(session, id, 2, { title: "stale", markdown: "旧 rev" }, 1),
    ).rejects.toMatchObject({ status: 409, code: "rev_conflict" });

    const page = await listNotes(session);
    expect(page.notes.some((note) => note.id === id)).toBe(true);

    const otherDevice = await enrollDevice(
      base,
      "",
      "unused",
      crypto.getRandomValues(new Uint8Array(32)),
    ).catch((error: unknown) => error);
    expect(otherDevice).toBeInstanceOf(Error);

    expect(() => decryptNote({ ...session, mk: generateKey() }, fetched)).toThrow();
  });
});
