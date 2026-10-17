# -*- coding: utf-8 -*-
"""
export_stage_lists.py — 从 stardict.db 导出学段词表（小学/初中/高中，+CET/考研顺手）

数据源：%APPDATA%/WinOCR/dict/ecdict/stardict.db 的 tag 字段
  zk = 中考课标（1603 词，义务教育五级）→ junior.txt
  gk = 高考课标（3677 词）→ senior.txt
  小学无直接标签 → zk ∩ CEFR A1/A2（义务教育二级 ~500 词的近似）→ primary.txt
输出：app/assets/dict/stage/{primary,junior,senior}.txt（每行一词，小写）
     进 assets 分发（文件小），sync_from_assets 同步到 %APPDATA%。

用法: python tools/export_stage_lists.py
"""
import os
import sqlite3

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DB = os.path.join(os.environ["APPDATA"], "WinOCR", "dict", "ecdict", "stardict.db")
CEFR = os.path.join(ROOT, "app", "assets", "terms", "cefr_full.txt")
DST = os.path.join(ROOT, "app", "assets", "dict", "stage")


def main():
    if not os.path.exists(DB):
        raise SystemExit("[!] 未找到 stardict.db，先跑 tools/install_ecdict.py")
    db = sqlite3.connect(DB)

    def tag_words(tag):
        return sorted(
            r[0].strip().lower()
            for r in db.execute("SELECT word FROM stardict WHERE tag LIKE ?", (f"%{tag}%",))
            if r[0].strip()
        )

    zk = tag_words("zk")
    gk = tag_words("gk")
    cet4 = tag_words("cet4")
    cet6 = tag_words("cet6")
    ky = tag_words("ky")

    # 小学 = 中考词 ∩ CEFR A1/A2（cefr_full.txt: word\tlevel）
    easy = set()
    if os.path.exists(CEFR):
        with open(CEFR, encoding="utf-8") as f:
            for line in f:
                parts = line.strip().split("\t")
                if len(parts) == 2 and parts[1] in ("A1", "A2"):
                    easy.add(parts[0].lower())
    primary = sorted(w for w in zk if w in easy)

    os.makedirs(DST, exist_ok=True)
    for name, words in [
        ("primary", primary),
        ("junior", zk),
        ("senior", gk),
        ("cet4", cet4),
        ("cet6", cet6),
        ("kaoyan", ky),
    ]:
        path = os.path.join(DST, f"{name}.txt")
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write("\n".join(words))
        print(f"[+] {name}: {len(words)} 词 → {path}")


if __name__ == "__main__":
    main()
