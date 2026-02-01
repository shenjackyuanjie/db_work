#!/usr/bin/env python3
import argparse
import base64
import mimetypes
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


def parse_disease_result(result_json: dict) -> tuple[bool, str]:
    """
    解析后端返回的诊断结果
    返回: (is_hlb, diagnosis_info) - 是否为黄龙病，诊断信息字符串
    """
    # /citrus/analyze 返回结构：{"success": true/false, "data": {...}, ...}
    data = result_json.get("data") if isinstance(result_json, dict) else None
    if not isinstance(data, dict):
        raise ValueError(f"unexpected response shape, no data: {result_json}")

    disease = data.get("disease_analysis")
    if not isinstance(disease, dict):
        raise ValueError(f"unexpected response shape, no disease_analysis: {result_json}")

    is_healthy = disease.get("is_healthy")
    disease_name = disease.get("disease_name", "")
    confidence = disease.get("confidence", "")

    # 构建诊断信息字符串
    if is_healthy is True:
        diagnosis_info = f"健康 (置信度: {confidence})" if confidence else "健康"
        return False, diagnosis_info
    if is_healthy is False:
        name = str(disease_name).strip().lower()
        diagnosis_info = f"{disease_name} (置信度: {confidence})" if confidence else str(disease_name)
        # 允许模型输出不同写法
        if "黄龙病" in disease_name:
            return True, diagnosis_info
        if "hlb" in name or "huanglongbing" in name:
            return True, diagnosis_info
        return False, diagnosis_info

    raise ValueError(f"unexpected is_healthy value: {is_healthy}, raw: {result_json}")


def is_hlb_from_result(result_json: dict) -> bool:
    """兼容旧接口"""
    is_hlb, _ = parse_disease_result(result_json)
    return is_hlb


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

    for idx, img_path in enumerate(images, start=1):
        try:
            data_url = to_data_url(img_path)
            # 不发送 message 字段，只发送图片数据
            payload = {
                "image": data_url,
            }
            resp = session.post(endpoint, json=payload, timeout=args.timeout)
            resp.raise_for_status()
            j = resp.json()

            # Rust 端成功时通常会有 success=true
            if isinstance(j, dict) and j.get("success") is False:
                raise ValueError(f"server returned success=false: {j}")

            is_hlb, diagnosis_info = parse_disease_result(j)
            if is_hlb:
                hlb += 1
            ok += 1
            
            # 每张图片处理后立即打印结果，包含后端诊断信息
            status = "HLB" if is_hlb else "OK"
            print(f"[{idx}/{total}] {status} - {img_path.name} | 诊断: {diagnosis_info}")
            
        except Exception as e:
            failed += 1
            # 输出到 stderr
            print(f"[{idx}/{total}] FAILED - {img_path.name}: {e}", file=sys.stderr)

    scanned = ok + failed
    rate = (hlb / ok) if ok > 0 else 0.0

    # 打印汇总信息
    print("\n" + "="*60)
    print("处理完成 - 汇总报告")
    print("="*60)
    print(f"dir={folder}")
    print(f"server={endpoint}")
    print(f"total_images={total}")
    print(f"processed={scanned}")
    print(f"ok={ok}")
    print(f"failed={failed}")
    print(f"hlb_images={hlb}")
    print(f"hlb_rate={rate:.6f}")
    print("="*60)

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
