#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""整句脏数据清洗（collect 表）· 2026-10-17 用户拍板「删整句，留短语」

判定：word 按空白切词 ≥5 个词，或总长 >40 字符 → 判为整句，删除。
2~4 词短语保留（renal failure 等医学短语有价值）。

用法：
  python tools/clean_sentence_collect.py            # 仅列清单（只读，不动库）
  python tools/clean_sentence_collect.py --apply    # 执行删除（先自动备份 DB）

注意：--apply 前请先关闭 WinOCR 主程序（避免 WAL 并发写冲突）。
输出统一 ASCII 前缀（[+] [-] [i]），词条内容原样打印。
"""
import os
import shutil
import sqlite3
import sys
from datetime import datetime

DB = os.path.expandvars(r"%APPDATA%\WinOCR\winocr.db")
MIN_WORDS = 5
MAX_CHARS = 40


def is_sentence(word):
    words = [t for t in word.split() if any(ch.isalpha() for ch in t)]
    return len(words) >= MIN_WORDS or len(word) > MAX_CHARS


def main():
    apply_mode = "--apply" in sys.argv
    if not os.path.exists(DB):
        print("[-] DB not found:", DB)
        sys.exit(1)
    c = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
    rows = c.execute("SELECT id, word FROM collect ORDER BY ts").fetchall()
    c.close()
    sentences = [(i, w) for i, w in rows if is_sentence(w)]

    print(f"[i] total={len(rows)}  sentence-like={len(sentences)}  "
          f"(>= {MIN_WORDS} words or > {MAX_CHARS} chars)")
    print("[i] ---- list ----")
    for i, (rid, w) in enumerate(sentences, 1):
        print(f"  {i:3d}. {w}")
        if i >= 60:
            print(f"  ... and {len(sentences) - 60} more")
            break

    if not apply_mode:
        print("[i] list-only mode. Run with --apply to delete (backup first, close app).")
        return

    # 备份
    ts = datetime.now().strftime("%Y%m%d-%H%M%S")
    bak = DB + f".bak-clean-{ts}"
    shutil.copy2(DB, bak)
    print(f"[+] backup -> {bak}")

    conn = sqlite3.connect(DB)
    cur = conn.cursor()
    n = 0
    for rid, w in sentences:
        cur.execute("DELETE FROM collect WHERE id = ?", (rid,))
        n += cur.rowcount
    conn.commit()
    conn.close()
    print(f"[+] deleted {n} sentence-like rows. Done.")


if __name__ == "__main__":
    main()
