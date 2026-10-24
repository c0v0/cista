import { describe, expect, it } from "vitest";

import { parseShareMeta, ShareClient, ShareError } from "./shareClient";

interface Call {
  url: string;
  init: RequestInit;
}

function recordingTransport(
  respond: (call: Call) => Response,
): { transport: typeof fetch; calls: Call[] } {
  const calls: Call[] = [];
  // fetch 的完整签名（RequestInfo | URL + RequestInit + Promise<Response>）无法用
  // 一个窄函数字面量表达，这里保留一次显式转换，仅用于测试替身。
  const transport = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const call: Call = { url: String(input), init: init ?? {} };
    calls.push(call);
    return respond(call);
  }) as typeof fetch;
  return { transport, calls };
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const TOKEN = "0123456789abcdefghij";

const META = {
  kind: "workspace",
  title: "共享工作区",
  note_scopes: 1,
  file_scopes: 4,
  expires_at: null,
  requires_passphrase: false,
  max_uses: null,
  used_count: 0,
};

describe("parseShareMeta", () => {
  it("拒绝缺字段/错类型的响应", () => {
    expect(() => parseShareMeta({ ...META, kind: "unknown" })).toThrow(/kind/);
    expect(() => parseShareMeta({ ...META, used_count: "0" })).toThrow(/used_count/);
    expect(() => parseShareMeta(null)).toThrow(/不是对象/);
    expect(parseShareMeta(META).title).toBe("共享工作区");
  });
});

describe("ShareClient", () => {
  it("token 只在 Authorization 头里，URL 不含 token", async () => {
    const { transport, calls } = recordingTransport(() => jsonResponse(200, META));
    const client = new ShareClient(transport, TOKEN);
    const meta = await client.meta("workspace");

    expect(meta.title).toBe("共享工作区");
    const call = calls[0];
    expect(call?.url).toBe("/public/workspaces/meta");
    const headers = new Headers(call?.init.headers);
    expect(headers.get("authorization")).toBe(`Share ${TOKEN}`);
    expect(call?.url.includes(TOKEN)).toBe(false);
  });

  it("拒绝把 token 拼进 path", async () => {
    const { transport } = recordingTransport(() => jsonResponse(200, META));
    const client = new ShareClient(transport, TOKEN);
    await expect(client.rename(TOKEN, "x.pdf")).rejects.toThrow(/token/);
  });

  it("有口令时附带 x-share-passphrase", async () => {
    const { transport, calls } = recordingTransport(() =>
      jsonResponse(200, { ticket: "t", expires_at: 1 }),
    );
    const client = new ShareClient(transport, TOKEN, { passphrase: "lettre" });
    await client.confirm();
    const call = calls[0];
    const headers = new Headers(call?.init.headers);
    expect(headers.get("x-share-passphrase")).toBe("lettre");
    expect(call?.init.method).toBe("POST");
  });

  it("410 映射成 ShareError(isGone)", async () => {
    const { transport } = recordingTransport(() => jsonResponse(410, { error: "gone" }));
    const client = new ShareClient(transport, TOKEN);
    const error: unknown = await client.meta("file").catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(ShareError);
    if (!(error instanceof ShareError)) throw new Error("expected ShareError");
    expect(error.status).toBe(410);
    expect(error.code).toBe("gone");
    expect(error.isGone).toBe(true);
  });

  it("上传带文件名与 MIME，下载返回 Blob", async () => {
    const entry = {
      id: "f1",
      name: "report.pdf",
      size: 3,
      mime: "application/pdf",
      rev: 1,
      created_at: 1,
    };
    const { transport, calls } = recordingTransport((call) =>
      call.url.endsWith("/upload")
        ? jsonResponse(201, entry)
        : new Response("abc", { status: 200 }),
    );
    const client = new ShareClient(transport, TOKEN);
    const uploaded = await client.upload(
      "report.pdf",
      new Blob(["abc"], { type: "application/pdf" }),
      "application/pdf",
    );
    expect(uploaded.name).toBe("report.pdf");

    const call = calls[0];
    const headers = new Headers(call?.init.headers);
    expect(headers.get("x-file-name")).toBe("report.pdf");
    expect(headers.get("content-type")).toBe("application/pdf");

    const blob = await client.downloadFile("f1");
    expect(await blob.text()).toBe("abc");
  });

  it("票据只出现在 blob 请求的 query 里", async () => {
    const { transport, calls } = recordingTransport(
      () => new Response("data", { status: 200 }),
    );
    const client = new ShareClient(transport, TOKEN);
    const blob = await client.downloadTicket("ticket abc");
    expect(await blob.text()).toBe("data");
    const call = calls[0];
    expect(call?.url).toBe("/public/files/blob?ticket=ticket%20abc");
    expect(call?.url.includes(TOKEN)).toBe(false);
  });
});
