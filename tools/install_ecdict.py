# -*- coding: utf-8 -*-
"""
install_ecdict.py — 安装 ECDICT 英汉词典数据（SQLite 直读，零转换）

数据源：skywind3000/ECDICT 官方 release ecdict-sqlite-28.zip（207MB，340 万词）
  直连 502 时走镜像: https://ghfast.top/<github-url>
安装位置：%APPDATA%/WinOCR/dict/ecdict/stardict.db（812MB，不进 exe 不随程序分发）

用法:
  python tools/install_ecdict.py                # 已有 zip 就解压，没有则先下载
  python tools/install_ecdict.py --download     # 强制重新下载
"""
import os
import sys
import zipfile
import urllib.request

ZIP_PATH = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        ".workbuddy", "outputs", "ecdict-sqlite-28.zip")
URL_GITHUB = "https://github.com/skywind3000/ECDICT/releases/download/1.0.28/ecdict-sqlite-28.zip"
URL_MIRROR = "https://ghfast.top/" + URL_GITHUB


def dst_dir():
    return os.path.join(os.environ["APPDATA"], "WinOCR", "dict", "ecdict")


def download(force=False):
    if os.path.exists(ZIP_PATH) and os.path.getsize(ZIP_PATH) == 216765132 and not force:
        print("[+] zip 已存在且大小正确，跳过下载")
        return
    for url in (URL_GITHUB, URL_MIRROR):
        try:
            print(f"[*] 下载 {url}")
            urllib.request.urlretrieve(url, ZIP_PATH)
            print(f"[+] 完成: {os.path.getsize(ZIP_PATH)} bytes")
            return
        except Exception as e:
            print(f"[-] 失败: {e}")
    sys.exit("[!] 两个地址都下载失败，请检查网络/代理")


def extract():
    dst = dst_dir()
    os.makedirs(dst, exist_ok=True)
    with zipfile.ZipFile(ZIP_PATH) as z:
        for n in z.namelist():
            target = os.path.join(dst, os.path.basename(n))
            with z.open(n) as src, open(target, "wb") as f:
                while True:
                    chunk = src.read(1 << 20)
                    if not chunk:
                        break
                    f.write(chunk)
            print(f"[+] 解压: {target} ({os.path.getsize(target)/1048576:.0f} MB)")


if __name__ == "__main__":
    if "--download" in sys.argv or not os.path.exists(ZIP_PATH):
        download(force="--download" in sys.argv)
    extract()
    print("[+] 安装完成，重启主程序后生效（学习词典切到「英汉释义 (ECDICT)」）")
