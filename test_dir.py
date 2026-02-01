#!/usr/bin/env python3
import argparse
import base64
import io
import mimetypes
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

import requests
from PIL import Image


IMAGE_EXTS = {".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff"}

# 并发数量控制：默认 5
CONCURRENCY = 20


def iter_images(folder: Path):
    for p in folder.rglob("*"):
        if p.is_file() and p.suffix.lower() in IMAGE_EXTS:
            yield p


def to_data_url(path: Path) -> str:
    """
    将图片转为 data URL。
    发送前如果图片尺寸任一边 > max_size，则等比例缩放到长宽都 <= max_size。
    有缩放时使用 Pillow 重新编码为 PNG（避免不同格式的编码/压缩差异带来的不确定性）。
    """
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
    max_size = 512
    with Image.open(path) as im:
        w, h = im.size
        if w > max_size or h > max_size:
            print(f"缩放: {path.name} 从 {w}x{h} 到 ", end="")
            im.thumbnail((max_size, max_size), resample=Image.Resampling.LANCZOS)
            print(f"{im.size[0]}x{im.size[1]}")

            out = io.BytesIO()
            im.save(out, format="PNG")
            raw = out.getvalue()
            mime = "image/png"
        else:
            raw = path.read_bytes()

    b64 = base64.b64encode(raw).decode("ascii")
    return f"data:{mime};base64,{b64}"

def parse_disease_result(result_json: dict) -> tuple[bool, str]:
    """
    解析后端返回的诊断结果（/citrus/analyze）
    后端结构（CitrusAnalysisResponse）:
      {
        "success": bool,
        "data": {
          "is_citrus_leaf": bool,
          "citrus_type": "...",
          "disease_analysis": {
            "is_healthy": bool,
            "disease_name": str,
            "severity": "...",
            "confidence": float,  # 0~1
            "treatment_suggestion": str,
            "preventive_measures": str
          },
          "image_quality_warning": str
        },
        "usage": {...},
        "metrics": {...}
      }

    返回: (is_hlb, diagnosis_info) - 是否为黄龙病，诊断信息字符串
    """
    if not isinstance(result_json, dict):
        raise ValueError(f"unexpected response type, expected dict: {type(result_json)}")

    success = result_json.get("success")
    if success is not True:
        # 后端失败时通常会返回 {"success": false, "error": "..."}
        raise ValueError(f"backend analyze failed: {result_json.get('error')}, raw: {result_json}")

    data = result_json.get("data")
    if not isinstance(data, dict):
        raise ValueError(f"unexpected response shape, no data: {result_json}")

    # 如果不是柑橘叶片，直接给出提示（避免误判病害）
    is_citrus_leaf = data.get("is_citrus_leaf")
    if is_citrus_leaf is False:
        citrus_type = data.get("citrus_type", "非柑橘")
        warning = str(data.get("image_quality_warning") or "").strip()
        info = f"非柑橘叶片 (识别类型: {citrus_type})"
        if warning:
            info = f"{info}；图片质量告警: {warning}"
        return False, info

    disease = data.get("disease_analysis")
    if not isinstance(disease, dict):
        raise ValueError(f"unexpected response shape, no disease_analysis: {result_json}")

    is_healthy = disease.get("is_healthy")
    disease_name = str(disease.get("disease_name") or "").strip()
    severity = str(disease.get("severity") or "").strip()
    confidence = disease.get("confidence")

    # 置信度展示：后端是 0~1 的 float，这里格式化为百分比更直观
    confidence_str = ""
    if isinstance(confidence, (int, float)):
        confidence_str = f"{confidence:.0%}"
    elif confidence is not None:
        confidence_str = str(confidence).strip()

    # 组装描述信息（把 severity / 治疗 / 预防 / 图片告警都带出来，方便排查与展示）
    treatment = str(disease.get("treatment_suggestion") or "").strip()
    prevention = str(disease.get("preventive_measures") or "").strip()
    warning = str(data.get("image_quality_warning") or "").strip()

    parts: list[str] = []
    if is_healthy is True:
        parts.append("健康")
        if confidence_str:
            parts.append(f"置信度: {confidence_str}")
        if warning:
            parts.append(f"图片质量告警: {warning}")
        return False, "；".join(parts)

    if is_healthy is False:
        if disease_name:
            parts.append(disease_name)
        else:
            parts.append("不健康")

        if severity:
            parts.append(f"程度: {severity}")
        if confidence_str:
            parts.append(f"置信度: {confidence_str}")
        if treatment:
            parts.append(f"治疗建议: {treatment}")
        if prevention:
            parts.append(f"预防措施: {prevention}")
        if warning:
            parts.append(f"图片质量告警: {warning}")

        diagnosis_info = "；".join(parts)

        # 判定是否黄龙病：兼容中文/英文缩写
        name_lower = disease_name.lower()
        if "黄龙病" in disease_name:
            return True, diagnosis_info
        if "hlb" in name_lower or "huanglongbing" in name_lower:
            return True, diagnosis_info
        return False, diagnosis_info

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

    ok = 0
    failed = 0
    hlb = 0

    def analyze_one(img_path: Path):
        # 每个任务使用自己的 Session，避免跨线程共享 Session 的不确定行为
        session = requests.Session()
        data_url = to_data_url(img_path)
        payload = {"image": data_url}
        resp = session.post(endpoint, json=payload, timeout=args.timeout)
        resp.raise_for_status()
        j = resp.json()

        # Rust 端成功时通常会有 success=true
        if isinstance(j, dict) and j.get("success") is False:
            raise ValueError(f"server returned success=false: {j}")

        is_hlb, diagnosis_info = parse_disease_result(j)
        return is_hlb, diagnosis_info

    futures = {}
    with ThreadPoolExecutor(max_workers=CONCURRENCY) as executor:
        for idx, img_path in enumerate(images, start=1):
            fut = executor.submit(analyze_one, img_path)
            futures[fut] = (idx, img_path)

        for fut in as_completed(futures):
            idx, img_path = futures[fut]
            try:
                is_hlb, diagnosis_info = fut.result()
                if is_hlb:
                    hlb += 1
                ok += 1

                status = "HLB" if is_hlb else "OK"
                scanned_now = ok + failed
                acc_now = (hlb / ok) if scanned_now > 0 else 0.0
                print(
                    f"[{idx}/{total}] {status} - {img_path.name} | 诊断: {diagnosis_info} | 当前正确率: {acc_now:.2%} ({hlb}/{ok})"
                )
            except Exception as e:
                failed += 1
                scanned_now = ok + failed
                acc_now = (hlb / ok) if scanned_now > 0 else 0.0
                print(
                    f"[{idx}/{total}] FAILED - {img_path.name}: {e} | 当前正确率: {acc_now:.2%} ({hlb}/{ok})",
                    file=sys.stderr,
                )

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
