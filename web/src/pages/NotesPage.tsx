import { useCallback, useEffect, useState } from "react";

import { desktopSecretStore } from "../lib/native";
import { decryptNote, listNotes, newNoteId, saveNote } from "../lib/notesApi";
import type { Session } from "../lib/notesApi";
import { ApiError } from "../lib/notesApi";
import { forgetRememberedDevice } from "../lib/rememberedDevice";
import { RecoveryCodePanel } from "./PairPage";

interface NoteSummary {
  id: string;
  rev: number;
  title: string;
}

/**
 * 笔记页（桌面与 Web 共用）：列表只解出标题，正文按需解密。
 * 保存失败（409）不覆盖服务端内容，只提示并刷新。
 */
export function NotesPage({
  session,
  onLock,
}: {
  session: Session;
  onLock?: () => void;
}) {
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [current, setCurrent] = useState<{ id: string; rev: number } | null>(null);
  const [title, setTitle] = useState("");
  const [markdown, setMarkdown] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const page = await listNotes(session);
      const summaries: NoteSummary[] = [];
      for (const record of page.notes) {
        let noteTitle = "(无法解密)";
        try {
          noteTitle = decryptNote(session, record).title || "(无标题)";
        } catch {
          // MK 不匹配（换了恢复码）时不该静默显示密文，这里保留占位。
        }
        summaries.push({ id: record.id, rev: record.rev, title: noteTitle });
      }
      setNotes(summaries);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "加载失败");
    }
  }, [session]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const open = (summary: NoteSummary) => {
    const record = { id: summary.id, rev: summary.rev };
    setCurrent(record);
    setStatus(null);
    void (async () => {
      try {
        const page = await listNotes(session);
        const found = page.notes.find((entry) => entry.id === summary.id);
        if (!found) {
          setError("笔记已不存在");
          return;
        }
        const plaintext = decryptNote(session, found);
        setCurrent({ id: found.id, rev: found.rev });
        setTitle(plaintext.title);
        setMarkdown(plaintext.markdown);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : "解密失败");
      }
    })();
  };

  const create = () => {
    setCurrent({ id: newNoteId(), rev: 0 });
    setTitle("");
    setMarkdown("");
    setStatus("新笔记：保存后 rev 从 1 开始");
    setError(null);
  };

  const save = async () => {
    if (!current) return;
    setBusy(true);
    setError(null);
    const nextRev = current.rev + 1;
    try {
      const saved = await saveNote(
        session,
        current.id,
        nextRev,
        { title, markdown },
        current.rev === 0 ? undefined : current.rev,
      );
      setCurrent({ id: saved.id, rev: saved.rev });
      setStatus(`已保存（rev ${saved.rev}）`);
      await refresh();
    } catch (cause) {
      if (cause instanceof ApiError && cause.isRevConflict) {
        setStatus(null);
        setError(`保存冲突：服务端已是 rev ${cause.rev ?? "?"}，已刷新列表，请重新打开再改`);
        await refresh();
      } else {
        setError(cause instanceof Error ? cause.message : "保存失败");
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <h1>我的笔记</h1>
      <p className="hint">
        设备 {session.device.id} · 端到端加密（服务端只存密文）
      </p>
      <RecoveryCodePanel mk={session.mk} />
      <div className="toolbar">
        <button
          type="button"
          onClick={() => {
            const store = desktopSecretStore();
            if (store) void forgetRememberedDevice(store);
            onLock?.();
          }}
        >
          锁定并清除本机记录
        </button>
      </div>

      <div className="toolbar">
        <button type="button" onClick={create}>
          新建笔记
        </button>
        <button type="button" disabled={busy} onClick={() => void refresh()}>
          刷新列表
        </button>
      </div>

      {notes.length > 0 ? (
        <ul className="files">
          {notes.map((summary) => (
            <li key={summary.id}>
              <span>
                {summary.title}
                <small>
                  {" "}
                  rev {summary.rev} · {summary.id.slice(0, 8)}
                </small>
              </span>
              <button type="button" onClick={() => open(summary)}>
                打开
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="hint">还没有笔记。</p>
      )}

      {current ? (
        <div className="editor">
          <input
            aria-label="标题"
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            placeholder="标题"
          />
          <textarea
            aria-label="正文"
            value={markdown}
            onChange={(event) => setMarkdown(event.target.value)}
            rows={14}
            placeholder="Markdown 正文；服务端不可读"
          />
          <button type="button" disabled={busy} onClick={() => void save()}>
            {busy ? "保存中…" : current.rev === 0 ? "创建" : "保存"}
          </button>
        </div>
      ) : null}

      {status ? <p className="hint">{status}</p> : null}
      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}
