/**
 * 客户端本地文件索引（Web 内存实现；语义与 `crates/core/src/search.rs` 一致）。
 *
 * 规则（`docs/specs/phase4-clients.md` §1）：
 * - 只存文件名/标题/ID，不存正文；不同步、不上传。
 * - 子串匹配（大小写不敏感），`%`/`_`/`\` 是字面量。
 * - 排序：`updatedAt` 新的在前，同名按 `name` 升序；默认 50、上限 200。
 */

export const SOURCE_NOTE_ATTACHMENT = "note_attachment";
export const SOURCE_DRIVE_NODE = "drive_node";

export const SEARCH_DEFAULT_LIMIT = 50;
export const SEARCH_MAX_LIMIT = 200;

export interface FileIndexRow {
  sourceType: string;
  sourceId: string;
  noteId: string | null;
  name: string;
  noteTitle: string | null;
  updatedAt: number;
}

/** SQL `LIKE … ESCAPE '\'` 的转义：`\` → `\\`，`%` → `\%`，`_` → `\_`。 */
export function escapeLike(raw: string): string {
  let escaped = "";
  for (const char of raw) {
    if (char === "\\" || char === "%" || char === "_") {
      escaped += `\\${char}`;
      continue;
    }
    escaped += char;
  }
  return escaped;
}

/** 与 `LIKE '%q%' ESCAPE '\'` 等价的大小写不敏感子串匹配。 */
export function likeContains(haystack: string, query: string): boolean {
  if (query.length === 0) return false;
  return haystack.toLowerCase().includes(query.toLowerCase());
}

/** 默认 50，硬上限 200（`design.md` §8）。 */
export function clampLimit(limit: number): number {
  if (!Number.isFinite(limit) || limit <= 0) return SEARCH_DEFAULT_LIMIT;
  return Math.min(Math.floor(limit), SEARCH_MAX_LIMIT);
}

export class LocalFileIndex {
  private rows: FileIndexRow[] = [];

  /** 插入或按 `(sourceType, sourceId)` 覆盖。 */
  upsert(row: FileIndexRow): void {
    const index = this.rows.findIndex(
      (existing) =>
        existing.sourceType === row.sourceType && existing.sourceId === row.sourceId,
    );
    if (index === -1) {
      this.rows.push(row);
      return;
    }
    this.rows[index] = row;
  }

  remove(sourceType: string, sourceId: string): void {
    this.rows = this.rows.filter(
      (row) => !(row.sourceType === sourceType && row.sourceId === sourceId),
    );
  }

  /** 笔记删除：级联移除该笔记下的索引项（不返回死链）。 */
  removeNote(noteId: string): void {
    this.rows = this.rows.filter((row) => row.noteId !== noteId);
  }

  /** 全量重建（索引可丢可重建）。 */
  rebuild(rows: FileIndexRow[]): void {
    this.rows = [...rows];
  }

  search(query: string, limit: number = SEARCH_DEFAULT_LIMIT): FileIndexRow[] {
    const trimmed = query.trim();
    if (trimmed.length === 0) return [];
    return this.rows
      .filter((row) => likeContains(row.name, trimmed))
      .sort((a, b) => {
        if (a.updatedAt !== b.updatedAt) return b.updatedAt - a.updatedAt;
        if (a.name < b.name) return -1;
        if (a.name > b.name) return 1;
        return 0;
      })
      .slice(0, clampLimit(limit));
  }

  get size(): number {
    return this.rows.length;
  }

  get isEmpty(): boolean {
    return this.rows.length === 0;
  }
}
