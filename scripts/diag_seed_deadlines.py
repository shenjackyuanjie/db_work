"""D16 复核：列出种子里所有「未来朝向」的截止时间及其余量。

回放的墙钟脆弱性 = 种子里存在「距 captured_at 很近的未来截止时间」。
本脚本把它们全部列出来，用于证明「只修一条」是有依据的，而不是漏看。

用法：
    python db\\scripts\\diag_seed_deadlines.py
"""

import datetime
import json
import pathlib

FIX = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "contract"

# 只关心可能被「与 now() 比较」的字段名。
DEADLINE_SUFFIXES = ("_at", "_start", "_end", "_date")


def is_deadline(field_name):
    return field_name.endswith(DEADLINE_SUFFIXES) or field_name in {
        "timestamp", "expires_at", "balance_due_at",
    }


def main():
    seed = json.loads((FIX / "seed.json").read_text(encoding="utf-8"))
    captured_at = datetime.datetime.fromisoformat(seed["_meta"]["captured_at"])

    print(f"captured_at = {captured_at.isoformat()}")
    print()
    print("=== 种子里「未来朝向」的截止时间（余量 = 值 - captured_at）===")

    rows = []
    naive = []
    for obj in seed["objects"]:
        for field, value in obj["fields"].items():
            if not is_deadline(field) or not isinstance(value, str):
                continue
            if "T" not in value:
                continue  # 纯日期串交给下面的「按期比较」段
            try:
                parsed = datetime.datetime.fromisoformat(value)
            except ValueError:
                continue
            if parsed.tzinfo is None:
                # 无偏移的**时间戳**（纯日期不算）——记下来，这本身是个观察点。
                naive.append((obj["model"], field, value))
                parsed = parsed.replace(tzinfo=datetime.timezone.utc)
            delta = parsed - captured_at
            if delta.total_seconds() > 0:
                rows.append((delta, obj["model"], field, value))

    rows.sort()
    if not rows:
        print("  （无未来朝向的截止时间）")
    for delta, model, field, value in rows:
        days = delta.total_seconds() / 86400
        flag = "  <<<< 短窗口" if delta.total_seconds() < 86400 else ""
        print(f"  +{days:>8.3f} 天  {model:<22} {field:<22} {value}{flag}")

    print()
    print("=== 未来朝向的纯日期字段（按期比较，天级）===")
    dates = []
    for obj in seed["objects"]:
        for field, value in obj["fields"].items():
            if not is_deadline(field) or not isinstance(value, str) or "T" in value:
                continue
            try:
                parsed = datetime.date.fromisoformat(value)
            except ValueError:
                continue
            delta = (parsed - captured_at.date()).days
            if delta > 0:
                dates.append((delta, obj["model"], field, value))
    dates.sort()
    for delta, model, field, value in dates:
        print(f"  +{delta:>4} 天  {model:<22} {field:<22} {value}")

    print()
    print("=== 最短未来窗口 ===")
    if rows:
        delta, model, field, value = rows[0]
        print(f"  {model}.{field} = {value}（+{delta.total_seconds() / 86400:.3f} 天）")

    print()
    print(f"=== 无时区偏移的朴素时间串（{len(naive)} 处）===")
    seen = {}
    for model, field, _value in naive:
        seen[(model, field)] = seen.get((model, field), 0) + 1
    for (model, field), count in sorted(seen.items()):
        print(f"  {model:<22} {field:<22} x{count}")


if __name__ == "__main__":
    main()
