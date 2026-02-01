#!/usr/bin/env python3
import argparse
import base64
import mimetypes
import os
import sys
from pathlib import Path

import requests


IMAGE_EXTS = {".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff"}


def iter_images(folder: Path):
    for p in folder.rglob("*"):
        if p.is_file() and p.suffix.lower() in IMAGE_EXTS:
            yield p


def to_data_url(path: Path) -> str:
    mime, _ = mimetypes.guess_type(str(path))
    if not mime:
        # fallback
        if path.suffix.lower() in {".jpg", ".jpeg"}:
            mime = "image/jpeg"
        elif path.suffix.lower() == ".png":
            mime = "image/png"
        elif path.suffix.lower() == ".gif":
            mime = "image/gif"
        elif path.suffix.lower() == ".webp":
            mime = "image/webp"
        else:
            mime = "application/octet-stream"

    raw = path.read_bytes()
    b64 = base64.b64encode(raw).decode("ascii")
    return f"data:{mime};base64,{b64}"


def is_hlb_from_result(result_json: dict) -> bool:
    # /citrus/analyze 返回结构：{"success": true/false, "data": {...}, ...}
    data = result_json.get("data") if isinstance(result_json, dict) else None
    if not isinstance(data, dict):
        raise ValueError(f"unexpected response shape, no data: {result_json}")

    disease = data.get("disease_analysis")
    if not isinstance(disease, dict):
        raise ValueError(f"unexpected response shape, no disease_analysis: {result_json}")

    is_healthy = disease.get("is_healthy")
    disease_name = disease.get("disease_name", "")

    if is_healthy is True:
        return False
    if is_healthy is False:
        name = str(disease_name).strip().lower()
        # 允许模型输出不同写法
        if "黄龙病" in disease_name:
            return True
        if "hlb" in name or "huanglongbing" in name:
            return True
        return False

    raise ValueError(f"unexpected is_healthy value: {is_healthy}, raw: {result_json}")


def main():
    parser = argparse.ArgumentParser(
        description="批量上传文件夹图片到 Rust GLM server，判定黄龙病并统计得病率（走 /citrus/analyze）"
    )
    parser.add_argument("--dir", required=True, help="本地图片文件夹路径")
    parser.add_argument(
        "--server",
        default="127.0.0.1:3000",
        help="服务器 ip:port，默认 127.0.0.1:3000",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=60.0,
        help="单张图片请求超时(秒)，默认 60",
    )
    args = parser.parse_args()

    folder = Path(args.dir).expanduser().resolve()
    if not folder.exists() or not folder.is_dir():
        print(f"dir not found or not a directory: {folder}", file=sys.stderr)
        return 2

    base_url = args.server
    if not (base_url.startswith("http://") or base_url.startswith("https://")):
        base_url = "http://" + base_url
    endpoint = base_url.rstrip("/") + "/citrus/analyze"

    images = list(iter_images(folder))
    total = len(images)
    if total == 0:
        print("total_images=0")
        print("hlb_images=0")
        print("hlb_rate=0.0")
        return 0

    session = requests.Session()

    ok = 0
    failed = 0
    hlb = 0

    # 这段 message 会被 server 用到（system prompt 已在 Rust 端固定）
    message = "请判断图片中的柑橘叶片/植株是否患黄龙病(HLB)。只需在 JSON 的 disease_analysis 里给出最可能疾病。"

    for idx, img_path in enumerate(images, start=1):
        try:
            data_url = to_data_url(img_path)
            payload = {
                "message": message,
                "image": data_url,
            }
            resp = session.post(endpoint, json=payload, timeout=args.timeout)
            resp.raise_for_status()
            j = resp.json()

            # Rust 端成功时通常会有 success=true
            if isinstance(j, dict) and j.get("success") is False:
                raise ValueError(f"server returned success=false: {j}")

            if is_hlb_from_result(j):
                hlb += 1
            ok += 1
        except Exception as e:
            failed += 1
            # 只输出到 stderr 避免污染最终统计
            print(f"[FAILED] {idx}/{total} {img_path}: {e}", file=sys.stderr)

    scanned = ok + failed
    rate = (hlb / ok) if ok > 0 else 0.0

    print(f"dir={folder}")
    print(f"server={endpoint}")
    print(f"total_images={total}")
    print(f"processed={scanned}")
    print(f"ok={ok}")
    print(f"failed={failed}")
    print(f"hlb_images={hlb}")
    print(f"hlb_rate={rate:.6f}")

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
