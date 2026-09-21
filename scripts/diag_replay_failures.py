"""打印一次回放报告里的失败用例与差异定位。

用法：
    python db\\scripts\\diag_replay_failures.py tests\\fixtures\\contract\\REPLAY_d16.json
"""

import json
import pathlib
import sys


def main():
    path = pathlib.Path(sys.argv[1] if len(sys.argv) > 1
                        else r"tests\fixtures\contract\REPLAY_final.json")
    report = json.loads(path.read_text(encoding="utf-8"))

    print(f"=== {path} ===")
    print(f"顶层键: {sorted(report)}")
    cases = report.get("cases") or report.get("results") or []
    if not cases:
        print("找不到用例数组")
        return

    failing = [c for c in cases if c.get("status") == "fail"]
    print(f"用例总数 {len(cases)}，失败 {len(failing)}")
    print()
    for case in failing:
        print(f"--- {case.get('domain')}/{case.get('name')} ---")
        for key, value in case.items():
            if key in {"name", "domain", "status"}:
                continue
            text = json.dumps(value, ensure_ascii=False)
            if len(text) > 900:
                text = text[:900] + f"…（共 {len(text)} 字符）"
            print(f"    {key}: {text}")
        print()


if __name__ == "__main__":
    main()
