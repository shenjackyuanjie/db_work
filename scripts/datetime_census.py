"""统计夹具里所有时间/数值形态，用于锁定序列化契约（不是猜，是实证）。"""

import json
import pathlib
import re
import sys
from collections import Counter

DT_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?$")
DATE_RE = re.compile(r"^\d{4}-\d{2}-\d{2}$")


def walk(node):
    if isinstance(node, dict):
        for value in node.values():
            yield from walk(value)
    elif isinstance(node, list):
        for value in node:
            yield from walk(value)
    else:
        yield node


for base in sys.argv[1:]:
    root = pathlib.Path(base)
    print("=" * 20, root)
    dt_forms = Counter()
    date_forms = 0
    decimal_like = Counter()
    for path in sorted(root.glob("*.json")):
        if path.name in {"index.json", "seed.json"}:
            continue
        doc = json.loads(path.read_text(encoding="utf-8"))
        for case in doc.get("cases", []):
            for value in walk(case.get("expected_body")):
                if not isinstance(value, str):
                    continue
                if DT_RE.match(value):
                    has_frac = "." in value.split("+")[0].split("Z")[0].split("T")[1]
                    frac_len = 0
                    if has_frac:
                        frac_len = len(value.split("T")[1].rstrip("Z").split("+")[0].split("-0")[0].split(".")[1])
                    suffix = "Z" if value.endswith("Z") else ("+00:00" if value.endswith("+00:00") else ("naive" if "-" not in value.split("T")[1] and "+" not in value.split("T")[1] else "other"))
                    dt_forms[(suffix, frac_len)] += 1
                elif DATE_RE.match(value):
                    date_forms += 1
                elif re.match(r"^-?\d+\.\d{1,2}$", value) and "." in value:
                    decimal_like[len(value.split(".")[1])] += 1

    print("  时间形态 (后缀, 小数位) -> 次数:")
    for key, count in dt_forms.most_common():
        print(f"    {key}: {count}")
    print(f"  纯日期串: {date_forms}")
    print(f"  小数位分布(疑似 Decimal/float 字符串): {dict(decimal_like)}")
