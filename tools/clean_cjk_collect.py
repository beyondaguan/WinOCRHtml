# -*- coding: utf-8 -*-
"""清理 collect 表中的中文污染行（只认英文采集后遗留的历史脏数据）。

背景：早期 split_english_tokens 用 char::is_alphabetic() 切词，对汉字返回 true，
导致整句中文被当成一个"词"写进生词本。已修复为 is_ascii_alphabetic()。
本脚本用于清理修复前遗留的脏数据。

用法：
  python clean_cjk_collect.py            # 预览待删除清单（不改动）
  python clean_cjk_collect.py --apply    # 实际删除（自动先备份 winocr.db）
"""
import os
import shutil
import sqlite3
import sys
import time

APPLY = "--apply" in sys.argv

db = os.path.expandvars(r"%APPDATA%\WinOCR\winocr.db")


def is_cjk(s):
    return any("\u4e00" <= c <= "\u9fff" for c in (s or ""))


def has_non_ascii_letters(s):
    """含非 ASCII 字母（中文/日文/西里尔等）——比纯 CJK 更宽的污染判定。"""
    for c in (s or ""):
        if c.isalpha() and not c.isascii():
            return True
    return False


con = sqlite3.connect(db)
cur = con.cursor()
cur.execute("SELECT id, ts, word, project FROM collect ORDER BY ts DESC")
rows = cur.fetchall()

bad = [r for r in rows if is_cjk(r[2])]
strict = [r for r in rows if has_non_ascii_letters(r[2])]

print("数据库:", db)
print("总行数: %d" % len(rows))
print("含 CJK 汉字的行: %d" % len(bad))
print("含非 ASCII 字母的行（含 CJK）: %d" % len(strict))
print()

if not strict:
    print("[+] 没有需要清理的污染数据")
    con.close()
    sys.exit(0)

print("=== 待删除清单 ===")
for i, (rid, ts, w, p) in enumerate(strict, 1):
    d = time.strftime("%Y-%m-%d %H:%M", time.localtime(ts))
    preview = w if len(w) <= 60 else w[:60] + "..."
    print("%2d. id=%s" % (i, rid))
    print("    时间=%s  分组=%s" % (d, p))
    print("    word=%r" % preview)

print()
if not APPLY:
    print("[预览模式] 未做任何改动。确认无误后执行：")
    print("    python clean_cjk_collect.py --apply")
    con.close()
    sys.exit(0)

# 备份
bak = db + ".bak-clean-%s" % time.strftime("%Y%m%d-%H%M%S")
shutil.copy2(db, bak)
print("[+] 已备份: %s (%d bytes)" % (bak, os.path.getsize(bak)))

ids = [(r[0],) for r in strict]
cur.executemany("DELETE FROM collect WHERE id = ?", ids)
con.commit()
cur.execute("SELECT COUNT(*) FROM collect")
left = cur.fetchone()[0]
con.close()

print("[+] 已删除 %d 行，剩余 %d 行（已提交的英文词条全部保留）" % (len(ids), left))