import { useCallback, useEffect, useState } from "react";

import {
  ShareClient,
  type PublicFileEntry,
  type ShareMeta,
} from "../lib/shareClient";
import type { ShareTarget } from "../lib/shareLink";
import {
  describeShareError,
  downloadName,
  filePermissions,
} from "./sharePageModel";

function saveBlob(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  anchor.rel = "noopener";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

/**
 * 分享落地页（访客端）。
 *
 * 不变量：token 只在 `ShareClient` 的 `Authorization` 头里；页面 URL 只有 fragment，
 * 不写 query/localStorage（`design.md` §3.2）。
 */
export function SharePage({ target }: { target: ShareTarget }) {
  const [meta, setMeta] = useState<ShareMeta | null>(null);
  const [files, setFiles] = useState<PublicFileEntry[]>([]);
  const [passphrase, setPassphrase] = useState("");
  const [needsPassphrase, setNeedsPassphrase] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(
    async (pass?: string) => {
      setBusy(true);
      setError(null);
      try {
        const client = new ShareClient(fetch, target.token, { passphrase: pass });
        const loaded = await client.meta(target.kind);
        setMeta(loaded);
        setNeedsPassphrase(loaded.requires_passphrase && !pass);
        if (
          target.kind === "workspace" &&
          filePermissions(loaded.file_scopes).canRead
        ) {
          setFiles(await client.listAllFiles());
        }
      } catch (cause) {
        setError(describeShareError(cause));
      } finally {
        setBusy(false);
      }
    },
    [target.kind, target.token],
  );

  useEffect(() => {
    void load();
  }, [load]);

  const downloadSingleFile = async () => {
    setBusy(true);
    setError(null);
    try {
      const client = new ShareClient(fetch, target.token, {
        passphrase: passphrase || undefined,
      });
      const confirmed = await client.confirm();
      const blob = await client.downloadTicket(confirmed.ticket);
      saveBlob(blob, downloadName(meta?.title ?? ""));
    } catch (cause) {
      setError(describeShareError(cause));
    } finally {
      setBusy(false);
    }
  };

  const downloadWorkspaceFile = async (entry: PublicFileEntry) => {
    setBusy(true);
    setError(null);
    try {
      const client = new ShareClient(fetch, target.token, {
        passphrase: passphrase || undefined,
      });
      saveBlob(await client.downloadFile(entry.id), downloadName(entry.name));
    } catch (cause) {
      setError(describeShareError(cause));
    } finally {
      setBusy(false);
    }
  };

  const uploadFile = async (file: File) => {
    setBusy(true);
    setError(null);
    try {
      const client = new ShareClient(fetch, target.token, {
        passphrase: passphrase || undefined,
      });
      await client.upload(file.name, file, file.type || "application/octet-stream");
      setFiles(await client.listAllFiles());
    } catch (cause) {
      setError(describeShareError(cause));
    } finally {
      setBusy(false);
    }
  };

  const permissions = meta ? filePermissions(meta.file_scopes) : null;

  return (
    <section>
      <h1>{meta?.title || (target.kind === "file" ? "分享的文件" : "分享的工作区")}</h1>

      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : null}

      {needsPassphrase ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void load(passphrase);
          }}
        >
          <label htmlFor="passphrase">此链接需要口令</label>
          <input
            id="passphrase"
            type="password"
            autoComplete="off"
            value={passphrase}
            onChange={(event) => setPassphrase(event.target.value)}
          />
          <button type="submit" disabled={busy || passphrase.length === 0}>
            解锁
          </button>
        </form>
      ) : null}

      {target.kind === "file" && meta && !needsPassphrase ? (
        <p>
          <button type="button" disabled={busy} onClick={() => void downloadSingleFile()}>
            {busy ? "准备中…" : "下载"}
          </button>
        </p>
      ) : null}

      {target.kind === "workspace" && permissions?.canRead && !needsPassphrase ? (
        <ul className="files">
          {files.length === 0 ? <li>工作区还没有文件。</li> : null}
          {files.map((entry) => (
            <li key={entry.id}>
              <span>{entry.name}</span>
              <button
                type="button"
                disabled={busy}
                onClick={() => void downloadWorkspaceFile(entry)}
              >
                下载
              </button>
            </li>
          ))}
        </ul>
      ) : null}

      {target.kind === "workspace" && permissions?.canUpload && !needsPassphrase ? (
        <label className="upload">
          上传文件
          <input
            type="file"
            disabled={busy}
            onChange={(event) => {
              const file = event.target.files?.[0];
              if (file) void uploadFile(file);
              event.target.value = "";
            }}
          />
        </label>
      ) : null}

      {!meta && !error ? <p>加载中…</p> : null}
    </section>
  );
}
