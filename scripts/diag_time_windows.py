"""D16 诊断：找出所有「被当前时间参与判定」的字段，并检查它们在夹具里是否被 normalize 屏蔽。

被判定 ≠ 被断言：只有**未被屏蔽**的字段才不允许动它的值。

用法：
    python db\\scripts\\diag_time_windows.py
"""

import json
import pathlib
from collections import defaultdict

FIX = pathlib.Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "contract"
DOMAINS = ["auth", "core", "commerce", "orchard_trace", "agent"]

# 会被 `timezone.now()` 参与判定的候选字段名（含 camelCase 变体）。
DEADLINE_KEYS = {
    "expires_at", "expiresAt",
    "balance_due_at", "balanceDueAt",
    "close_at", "closeAt",
    "open_at", "openAt",
    "is_open", "isOpen",
    "checkedAt", "checked_at",
    "cancelled_at", "cancelledAt",
    "paid_at", "paidAt",
    "verified_at", "verifiedAt",
    "status_display", "statusDisplay",
}


def walk(node, path=None):
    path = path or []
    if isinstance(node, dict):
        for key, value in node.items():
            yield from walk(value, path + [key])
    elif isinstance(node, list):
        for item in node:
            yield from walk(item, path + [key_placeholder])
    else:
        yield path, node


key_placeholder = "[]"


def masked_keys(normalize):
    """把 normalize 表达式归约成「被整体屏蔽的键名」集合。

    只处理 `$..key` 这种 roll-up 与 `$.a.b.key` 这种具体路径的尾键；
    够本诊断用（要判断的是「这个键有没有被屏蔽」）。
    """
    keys = set()
    for expr in normalize or []:
        for part in str(expr).split("|"):
            part = part.strip()
            if not part:
                continue
            if part in ("$", "$."):
                keys.add("*")
                continue
            if ".." in part:
                keys.add(part.split("..", 1)[1].strip("."))
            else:
                keys.add(part.rsplit(".", 1)[-1].strip("[]*"))
    return keys


def main():
    stats = defaultdict(lambda: {"cases": 0, "masked": 0, "unmasked_cases": []})

    for domain in DOMAINS:
        doc = json.loads((FIX / f"{domain}.json").read_text(encoding="utf-8"))
        for case in doc["cases"]:
            body = case.get("expected_body")
            if body is None:
                continue
            covered = masked_keys(case.get("normalize"))
            hits = set()
            for path, _value in walk(body):
                if not path:
                    continue
                key = path[-1]
                if key in DEADLINE_KEYS:
                    hits.add(key)
            for key in hits:
                entry = stats[key]
                entry["cases"] += 1
                if "*" in covered or key in covered:
                    entry["masked"] += 1
                else:
                    entry["unmasked_cases"].append(f"{domain}/{case['name']}")

    print("=== 被「当前时间」参与判定的字段：在夹具里的暴露面 ===")
    print(f"{'字段':<18}{'出现用例':>8}{'已屏蔽':>8}  未屏蔽用例")
    for key in sorted(stats, key=lambda k: -len(stats[k]["unmasked_cases"])):
        entry = stats[key]
        flag = "" if not entry["unmasked_cases"] else "   <<<< 未屏蔽！"
        print(f"{key:<18}{entry['cases']:>8}{entry['masked']:>8}{flag}")
        if entry["unmasked_cases"]:
            for name in entry["unmasked_cases"][:6]:
                print(f"      - {name}")
            if len(entry["unmasked_cases"]) > 6:
                print(f"      ... 共 {len(entry['unmasked_cases'])} 条")


if __name__ == "__main__":
    main()
