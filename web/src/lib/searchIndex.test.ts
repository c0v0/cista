import { describe, expect, it } from "vitest";

import {
  clampLimit,
  escapeLike,
  likeContains,
  LocalFileIndex,
  SEARCH_DEFAULT_LIMIT,
  SEARCH_MAX_LIMIT,
  type FileIndexRow,
} from "./searchIndex";

function row(
  sourceId: string,
  noteId: string | null,
  name: string,
  updatedAt: number,
): FileIndexRow {
  return {
    sourceType: "note_attachment",
    sourceId,
    noteId,
    name,
    noteTitle: noteId ? "周报" : null,
    updatedAt,
  };
}

describe("escapeLike", () => {
  it("转义 SQL 通配符与转义符本身", () => {
    expect(escapeLike("100%")).toBe("100\\%");
    expect(escapeLike("a_b")).toBe("a\\_b");
    expect(escapeLike("c:\\tmp")).toBe("c:\\\\tmp");
    expect(escapeLike("正常")).toBe("正常");
  });
});

describe("likeContains", () => {
  it("把 % 和 _ 当字面量，不误命中", () => {
    expect(likeContains("100%_draft.pdf", "100%_")).toBe(true);
    expect(likeContains("abc.pdf", "%")).toBe(false);
  });

  it("大小写不敏感且空查询不匹配", () => {
    expect(likeContains("Report.PDF", "report")).toBe(true);
    expect(likeContains("abc", "")).toBe(false);
  });
});

describe("clampLimit", () => {
  it("默认 50，硬上限 200", () => {
    expect(clampLimit(0)).toBe(SEARCH_DEFAULT_LIMIT);
    expect(clampLimit(-5)).toBe(SEARCH_DEFAULT_LIMIT);
    expect(clampLimit(Number.NaN)).toBe(SEARCH_DEFAULT_LIMIT);
    expect(clampLimit(10)).toBe(10);
    expect(clampLimit(10_000)).toBe(SEARCH_MAX_LIMIT);
  });
});

describe("LocalFileIndex", () => {
  it("按更新时间倒序、同名按名称升序、limit 生效", () => {
    const index = new LocalFileIndex();
    index.upsert(row("a1", "n1", "报价单.pdf", 300));
    index.upsert(row("a2", "n1", "报价单-v2.pdf", 200));
    index.upsert(row("a3", "n2", "会议记录.md", 400));

    const hits = index.search("报价");
    expect(hits.map((hit) => hit.sourceId)).toEqual(["a1", "a2"]);
    expect(index.search("报价", 1)).toHaveLength(1);
    expect(index.search("   ")).toEqual([]);
  });

  it("upsert 覆盖同一 source，改名立即生效", () => {
    const index = new LocalFileIndex();
    index.upsert(row("a1", "n1", "旧名.pdf", 1));
    index.upsert(row("a1", "n1", "新名.pdf", 2));
    expect(index.size).toBe(1);
    expect(index.search("新名")[0]?.name).toBe("新名.pdf");
  });

  it("删除笔记级联移除索引，不留死链", () => {
    const index = new LocalFileIndex();
    index.upsert(row("a1", "n1", "附件1.pdf", 1));
    index.upsert(row("a2", "n1", "附件2.pdf", 2));
    index.upsert(row("a3", "n2", "别的.pdf", 3));
    index.removeNote("n1");
    expect(index.size).toBe(1);
    expect(index.search("附件")).toEqual([]);
  });

  it("remove 只影响同 sourceType", () => {
    const index = new LocalFileIndex();
    index.upsert(row("x1", "n1", "同名.pdf", 1));
    index.upsert({
      sourceType: "drive_node",
      sourceId: "x1",
      noteId: null,
      name: "同名.pdf",
      noteTitle: null,
      updatedAt: 2,
    });
    index.remove("note_attachment", "x1");
    expect(index.size).toBe(1);
    expect(index.search("同名")[0]?.sourceType).toBe("drive_node");
  });

  it("rebuild 全量替换", () => {
    const index = new LocalFileIndex();
    index.upsert(row("a1", "n1", "x.pdf", 1));
    index.rebuild([row("b1", "n2", "y.pdf", 2)]);
    expect(index.size).toBe(1);
    expect(index.search("y")[0]?.sourceId).toBe("b1");
  });

  it("drive_node 行没有笔记绑定", () => {
    const index = new LocalFileIndex();
    index.upsert({
      sourceType: "drive_node",
      sourceId: "d1",
      noteId: null,
      name: "归档.zip",
      noteTitle: null,
      updatedAt: 5,
    });
    const hits = index.search("归档");
    expect(hits).toHaveLength(1);
    expect(hits[0]?.noteId).toBeNull();
  });

  it("空索引 isEmpty", () => {
    const index = new LocalFileIndex();
    expect(index.isEmpty).toBe(true);
    index.upsert(row("a1", "n1", "x.pdf", 1));
    expect(index.isEmpty).toBe(false);
  });
});
