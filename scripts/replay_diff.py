#!/usr/bin/env python
"""契约回放与比对：把 golden fixtures 打到 Rust 影子 `/compat` 上，逐条比对响应。

夹具语义（**逐条从 `db/scripts/capture_contract.py` 核对得出，不是猜的**）
--------------------------------------------------------------------------
1. `expected_body` 存的是**原始真值**（线上字节的 JSON 解析），**不是占位符**。
   `REPORT.md §9` / `DELIVERY_NOTE.md §3.1` 说"值已被替换为 `<sha256:16hex>`"与实测不符：
   实测 251 条用例里 0 条含 `<sha256:`。所以比对必须自己套 `normalize` 屏蔽。
2. `normalize` 是**已展开的 JSON 路径清单**，语法三种：
   - `$.a.b`        从根按字段走
   - `$.a[*].b`     数组内每个元素
   - `$..key`       任意深度的字段 key（roll-up）
   命中即把值替换为 `<sha256:…>`；比对时把占位符当**通配**（两次请求的哈希必然不同）。
3. 回放顺序：**每域 read 在前、mutation 在后**，域顺序
   `auth → core → commerce → orchard_trace → agent`；case 数组顺序就是录制顺序。
   `index.json.mutation_order[].order` 是写类的全局序号（字段名是 `order`，不是 `seq`）。
4. 固定 token 用 `uuid5(NAMESPACE_URL, "contract:<key>")` 派生（见 `Ctx.install_token`），
   key ∈ {farmer_xinfeng, farmer_xunwu, buyer_zhang, scratch, scratch2}。
   录制时**每个写类用例前**都会 `ensure_tokens()` 重建这 5 个 token，回放必须照做。
5. `expected_deviation`：`expected_status >= 500`（同时带 `case.server_error`）。
   这 12 条是蓝本缺陷（`DEVIATIONS.md` 的 D1/D2/D5），**已裁定 Rust 侧修正为正确语义**，
   因此不计失败。
6. `Z` 与 `+00:00` 视为等价，但单独计数输出到 `offset_style_drift`（D4）。

用法
----
    python db\\scripts\\replay_diff.py --schema compat_test
    python db\\scripts\\replay_diff.py --schema compat_test --domain auth
    python db\\scripts\\replay_diff.py --schema compat_test --case me_ok_buyer
    python db\\scripts\\replay_diff.py --self-check          # 只验比对器自身，不连服务
"""

from __future__ import annotations

import argparse
import base64
import copy
import datetime as dt
import hashlib
import json
import re
import subprocess
import sys
import uuid
import zlib
from pathlib import Path
from urllib.parse import urlencode

DB_ROOT = Path(__file__).resolve().parent.parent
FIXTURES = DB_ROOT / "tests" / "fixtures" / "contract"
CONFIG = DB_ROOT / "config.toml"

DOMAIN_ORDER = ["auth", "core", "commerce", "orchard_trace", "agent"]

# 录制器里 Ctx.install_token 的 key 名单（role 标签与 key 同名）。
TOKEN_KEYS = ["farmer_xinfeng", "farmer_xunwu", "buyer_zhang", "scratch", "scratch2"]
# key -> seed 里的真实 username。录制器 `ensure_tokens` 的映射就是这个，别用 key 当用户名。
TOKEN_USERNAME = {
    "farmer_xinfeng": "farmer_xinfeng",
    "farmer_xunwu": "farmer_xunwu",
    "buyer_zhang": "buyer_zhang",
    "scratch": "qa_scratch_user",
    "scratch2": "qa_scratch_user2",
}
TOKEN_NAMESPACE = uuid.NAMESPACE_URL
TOKEN_PREFIX = "contract:"
TOKEN_TTL_DAYS = 365

PLACEHOLDER = re.compile(r"^<sha256:[0-9a-f]{16}>$")
ISO_TZ = re.compile(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?)(Z|\+00:00)$")


def token_for(key: str) -> str:
    return str(uuid.uuid5(TOKEN_NAMESPACE, TOKEN_PREFIX + key))


# --------------------------------------------------------------------------------------
# 夹具加载
# --------------------------------------------------------------------------------------


def load_fixtures(fixtures: Path) -> tuple[dict, dict[str, dict]]:
    index = json.loads((fixtures / "index.json").read_text(encoding="utf-8"))
    domains: dict[str, dict] = {}
    for name in DOMAIN_ORDER:
        path = fixtures / f"{name}.json"
        if path.exists():
            domains[name] = json.loads(path.read_text(encoding="utf-8"))
    return index, domains


def seed_phase(index: dict, fixtures: Path) -> dict:
    """判断 seed.json 是「回放前」还是「回放后（末态）」。

    判据是实测的：`index.captured_values` 记的是**回放过程中**从响应里抓到的 id；
    如果这些 id 出现在 seed.json 里，说明 seed 是在回放结束后导出的。
    """
    seed_path = fixtures / "seed.json"
    result = {"path": str(seed_path), "phase": "unknown", "evidence": []}
    if not seed_path.exists():
        return result

    seed = json.loads(seed_path.read_text(encoding="utf-8"))
    pks = {str(o.get("pk")) for o in seed.get("objects", [])}
    captured = index.get("captured_values") or {}
    hits = sorted(k for k, v in captured.items() if str(v) in pks)

    body = seed_path.read_bytes()
    result["signature"] = "sha256:" + hashlib.sha256(body).hexdigest()
    result["expected_signature"] = index.get("seed_signature")
    if hits:
        result["phase"] = "post-replay"
        result["evidence"] = [f"captured_values.{k} 出现在 seed.json 里" for k in hits]
    else:
        result["phase"] = "pre-replay"
    return result


def mutation_order_crosscheck(index: dict, domains: dict[str, dict]) -> list[str]:
    """确认 case 数组顺序里的写类子序列与 index.mutation_order 一致。"""
    problems: list[str] = []
    seen: list[tuple[str, str]] = []
    for domain in DOMAIN_ORDER:
        payload = domains.get(domain)
        if not payload:
            continue
        for case in payload["cases"]:
            if case.get("kind") == "mutation":
                seen.append((domain, case["name"]))
    recorded = [(m["domain"], m["case"]) for m in index.get("mutation_order", [])]
    if seen != recorded:
        problems.append(
            "case 顺序里的写类子序列与 index.mutation_order 不一致："
            f"文件 {len(seen)} 条 / index {len(recorded)} 条"
        )
    return problems


# --------------------------------------------------------------------------------------
# normalize（与 capture_contract.py 的 apply_normalize 同语义）
# --------------------------------------------------------------------------------------


def parse_expr(expr: str):
    rest = expr[1:] if expr.startswith("$") else expr
    # 只有 "$..key"（双点）才是 rollup（任意层级按键名全量屏蔽）。
    # 写成单点会把 "$.data.order_number" 误判成 rollup，key 变成 "data.order_number"，
    # 于是所有多级路径的屏蔽**静默失效**——四个域都踩过这个坑。
    if rest.startswith(".."):
        return [("rollup", rest[2:])]
    segs: list[tuple] = []
    i = 0
    while i < len(rest):
        if rest[i] == ".":
            i += 1
            continue
        if rest.startswith("[*]", i):
            segs.append(("iter",))
            i += 3
            continue
        j = i
        while j < len(rest) and rest[j] not in ".[":
            j += 1
        if j > i:
            segs.append(("key", rest[i:j]))
        i = j
    return segs


def redact(value):
    payload = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return "<sha256:" + hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16] + ">"


def walk_segments(node, segs) -> None:
    if not segs:
        return
    head, tail = segs[0], segs[1:]
    if head[0] == "key":
        name = head[1]
        if isinstance(node, dict) and name in node:
            if not tail:
                node[name] = redact(node[name])
            else:
                walk_segments(node[name], tail)
    else:
        if isinstance(node, list):
            for item in node:
                walk_segments(item, tail)
        elif isinstance(node, dict):
            walk_segments(node, tail)


def rollup(node, key: str) -> None:
    if isinstance(node, dict):
        for k, v in list(node.items()):
            if k == key:
                node[k] = redact(v)
            else:
                rollup(v, key)
    elif isinstance(node, list):
        for item in node:
            rollup(item, key)


def apply_normalize(body, exprs):
    """返回屏蔽后的深拷贝（不就地改输入）。"""
    out = copy.deepcopy(body)
    for expr in exprs or []:
        for part in expr.split("|"):
            part = part.strip()
            if not part:
                continue
            segs = parse_expr(part)
            if segs and segs[0][0] == "rollup":
                rollup(out, segs[0][1])
            else:
                walk_segments(out, segs)
    return out


# --------------------------------------------------------------------------------------
# 比对
# --------------------------------------------------------------------------------------


def kind(value) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, int):
        return "int"
    if isinstance(value, float):
        return "float"
    if isinstance(value, str):
        return "str"
    if isinstance(value, list):
        return "list"
    return "dict"


def is_iso(value) -> bool:
    return isinstance(value, str) and ISO_TZ.match(value) is not None


def normalized_tz(value: str) -> str:
    m = ISO_TZ.match(value)
    return m.group(1) + "+00:00"


def compare(expected, actual, path="$", diffs=None, drift=None, max_diff=12):
    diffs = [] if diffs is None else diffs
    drift = [] if drift is None else drift
    if len(diffs) >= max_diff:
        return diffs, drift

    # 占位符当通配：**任一侧**是占位符即视为等价（键存在性仍然要比）。
    if isinstance(expected, str) and PLACEHOLDER.match(expected):
        return diffs, drift
    if isinstance(actual, str) and PLACEHOLDER.match(actual):
        return diffs, drift

    ek, ak = kind(expected), kind(actual)
    if ek != ak:
        # int/float 是不同形状：Django 的 FloatField 一定是 float，不做宽松合并。
        diffs.append(f"{path}: 类型 {ek} != {ak}（期望 {short(expected)} / 实际 {short(actual)}）")
        return diffs, drift

    if ek == "dict":
        for key in sorted(set(expected) | set(actual)):
            if key not in expected:
                diffs.append(f"{path}.{key}: 实际多出该键（值 {short(actual[key])}）")
            elif key not in actual:
                diffs.append(f"{path}.{key}: 实际缺少该键（期望 {short(expected[key])}）")
            else:
                compare(expected[key], actual[key], f"{path}.{key}", diffs, drift, max_diff)
            if len(diffs) >= max_diff:
                return diffs, drift
        return diffs, drift

    if ek == "list":
        if len(expected) != len(actual):
            diffs.append(f"{path}: 数组长度 {len(expected)} != {len(actual)}")
            return diffs, drift
        for i, (e, a) in enumerate(zip(expected, actual)):
            compare(e, a, f"{path}[{i}]", diffs, drift, max_diff)
            if len(diffs) >= max_diff:
                return diffs, drift
        return diffs, drift

    if expected != actual:
        if is_iso(expected) and is_iso(actual):
            if normalized_tz(expected) == normalized_tz(actual):
                drift.append(path)
                return diffs, drift
        diffs.append(f"{path}: {short(expected)} != {short(actual)}")
    return diffs, drift


def short(value, limit=90) -> str:
    text = json.dumps(value, ensure_ascii=False)
    return text if len(text) <= limit else text[: limit - 1] + "…"


# --------------------------------------------------------------------------------------
# shape_only 校验（键与类型，不比数值）
# --------------------------------------------------------------------------------------


def resolve_shape_path(body, path: str) -> list:
    """解析 `$.a.b[*].c`，`[*]` 展开成每个元素。返回值列表（可能为空=不存在）。"""
    segs = []
    for tok in re.findall(r"\.([A-Za-z0-9_]+)|\[\*\]|\[(\d+)\]", path.lstrip("$")):
        name, index = tok
        if name:
            segs.append(("key", name))
        elif index:
            segs.append(("idx", int(index)))
        else:
            segs.append(("iter", None))

    def walk(node, rest):
        if not rest:
            return [node]
        head, tail = rest[0], rest[1:]
        if head[0] == "key":
            if isinstance(node, dict) and head[1] in node:
                return walk(node[head[1]], tail)
            return []
        if head[0] == "idx":
            if isinstance(node, list) and head[1] < len(node):
                return walk(node[head[1]], tail)
            return []
        out = []
        if isinstance(node, list):
            for item in node:
                out.extend(walk(item, tail))
        return out

    return walk(body, segs)


SHAPE_TYPES = {
    "int": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "float": lambda v: isinstance(v, float),
    "str": lambda v: isinstance(v, str),
    "bool": lambda v: isinstance(v, bool),
    "null": lambda v: v is None,
    "dict": lambda v: isinstance(v, dict),
    "list": lambda v: isinstance(v, list),
}


def check_shape(body, shape: list[str]) -> list[str]:
    problems: list[str] = []
    for item in shape:
        path, _, want = item.rpartition(":")
        if not path or want not in SHAPE_TYPES:
            problems.append(f"{item}: 无法解析的 shape 项")
            continue
        found = resolve_shape_path(body, path)
        if not found:
            problems.append(f"{path}: 路径不存在（期望 {want}）")
            continue
        for value in found:
            if not SHAPE_TYPES[want](value):
                problems.append(f"{path}: 期望 {want}，实际 {kind(value)}（{short(value)}）")
                break
    return problems


# --------------------------------------------------------------------------------------
# 请求构造
# --------------------------------------------------------------------------------------

_PNG: bytes | None = None


def make_png_8x8() -> bytes:
    """与 capture_contract.py 逐字节一致（夹具里的 sha256/size 就是它的指纹）。"""
    global _PNG
    if _PNG is not None:
        return _PNG

    def chunk(tag: bytes, payload: bytes) -> bytes:
        return (
            len(payload).to_bytes(4, "big")
            + tag
            + payload
            + (zlib.crc32(tag + payload) & 0xFFFFFFFF).to_bytes(4, "big")
        )

    width = height = 8
    rows = []
    for y in range(height):
        row = b"\x00"
        for x in range(width):
            row += bytes([(x * 32) % 256, 160, (y * 32) % 256])
        rows.append(row)
    ihdr = width.to_bytes(4, "big") + height.to_bytes(4, "big") + bytes([8, 2, 0, 0, 0])
    _PNG = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
        + chunk(b"IEND", b"")
    )
    return _PNG


def build_request(case: dict, base_url: str):
    path = case["path"]
    query = case.get("query") or {}
    if case["method"] == "GET" and query:
        path = f"{path}?{urlencode(query)}"

    headers = {}
    auth = case["headers"]["auth"]
    if auth == "malformed":
        headers["Authorization"] = "Token not-bearer"
    elif auth == "unknown":
        headers["Authorization"] = f"Bearer {uuid.uuid4()}"
    elif auth == "none":
        pass
    else:
        role = case["headers"]["role"] or "farmer_xinfeng"
        headers["Authorization"] = f"Bearer {token_for(role)}"

    body = case.get("body")
    if body is not None and body.get("IMAGE") == "@png_data_uri":
        body = dict(body, IMAGE="data:image/png;base64," + base64.b64encode(make_png_8x8()).decode())

    return base_url + path, headers, body


# --------------------------------------------------------------------------------------
# 令牌安装（复刻 Ctx.ensure_tokens / drop_token）
# --------------------------------------------------------------------------------------


class TokenStore:
    """回放过程中按需重建固定 token；`--no-db` 时退化为空操作。"""

    def __init__(self, dsn: str | None, schema: str | None):
        self.dsn = dsn
        self.schema = schema
        self.enabled = bool(dsn and schema)
        self.log: list[str] = []

    def _sql(self, statements: list[str]) -> None:
        psql = find_pg_tool("psql")
        proc = subprocess.run(
            [psql, self.dsn, "-v", "ON_ERROR_STOP=1", "-q", "-c", "\n".join(statements)],
            capture_output=True, text=True, encoding="utf-8",
        )
        if proc.returncode != 0:
            raise SystemExit(f"psql 失败（token 操作）:\n{proc.stderr or proc.stdout}")

    def ensure_scratch_users(self) -> None:
        """补上录制器在跑用例**之前**建的两个 scratch 账号。

        它们由 `run_capture` 里的两次 `POST /api/register` 建出来，之后才 `dump_seed`，
        但 seed.json 是**回放前**导出的纯种子态，所以这两个账号不在里面——而
        `install_all()` 是「按 username 查 id 再 INSERT」，账号不存在就静默插 0 行，
        scratch 相关用例于是恒 401。这里按录制时的语义补建（role=buyer）。
        """
        if not self.enabled:
            return
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        # 口令复用 seed 里 farmer_xinfeng 的 pbkdf2 串（口令 farmer123），
        # 只是为了账号本身合法可登录；本域用例不会用它们登录。
        password = (
            "pbkdf2_sha256$720000$9v4pzPFovtucXyIxzowUwi$"
            "x8sLz0Z+Q4brqnHHJSZK+rJ826ILbbSfyPwPzjwjAy0="
        )
        stmts = []
        for key in TOKEN_KEYS:
            username = TOKEN_USERNAME.get(key)
            if not username:
                continue
            stmts.append(
                'INSERT INTO "user" (id, username, password, email, role, orchard_address, '
                "latitude, longitude, created_at, updated_at) "
                f"SELECT '{uuid.uuid4()}', '{username}', '{password}', '{username}@example.com', "
                f"'buyer', NULL, NULL, NULL, '{now}', '{now}' "
                f"WHERE NOT EXISTS (SELECT 1 FROM \"user\" WHERE username = '{username}');"
            )
        if stmts:
            self._sql(stmts)
            self.log.append("ensure_scratch_users")

    def install_all(self) -> None:
        if not self.enabled:
            return
        now = dt.datetime.now(dt.timezone.utc).isoformat()
        expires = (dt.datetime.now(dt.timezone.utc) + dt.timedelta(days=TOKEN_TTL_DAYS)).isoformat()
        stmts = []
        for key in TOKEN_KEYS:
            # `role` 标签与 seed 里的 username **并不总是同名**：scratch -> qa_scratch_user。
            username = TOKEN_USERNAME.get(key, key)
            stmts.append(
                f'DELETE FROM auth_token WHERE user_id = '
                f'(SELECT id FROM "user" WHERE username = \'{username}\');'
            )
            stmts.append(
                f'INSERT INTO auth_token (key, user_id, created_at, expires_at) SELECT '
                f"'{token_for(key)}', id, '{now}', '{expires}' FROM \"user\" WHERE username = '{username}';"
            )
        self._sql(stmts)
        self.log.append("install_tokens")

    def drop(self, key: str) -> None:
        if not self.enabled:
            return
        self._sql([f"DELETE FROM auth_token WHERE key = '{token_for(key)}';"])
        self.log.append(f"drop_token:{key}")


def find_pg_tool(name: str) -> str:
    import shutil

    found = shutil.which(name)
    if found:
        return found
    for cand in (
        Path(r"D:\apps\pg\18\bin") / f"{name}.exe",
        Path(r"C:\Program Files\PostgreSQL\18\bin") / f"{name}.exe",
    ):
        if cand.exists():
            return str(cand)
    raise SystemExit(f"找不到 {name}：请放进 PATH，或设置 PG_BIN")


def base_dsn() -> str:
    text = CONFIG.read_text(encoding="utf-8")
    m = re.search(r'(?m)^\s*postgres_url\s*=\s*"([^"]+)"', text)
    if not m:
        raise SystemExit(f"{CONFIG} 里没有 [database].postgres_url")
    return m.group(1)


def schema_dsn(dsn: str, schema: str) -> str:
    assert_compat_schema(schema)
    opt = "options=-csearch_path%3D" + schema
    return f"{dsn}&{opt}" if "?" in dsn else f"{dsn}?{opt}"


def assert_compat_schema(schema: str) -> None:
    """回放只在 scratch schema 里动 token，绝不碰现网 public。"""
    if schema == "public":
        raise SystemExit(
            "拒绝 --schema public：回放会重建 auth_token，而现网 app_*/store_*/commerce_* "
            "有真实数据（6 用户 / 143 任务 / 20 棵树），任何写操作都禁止。"
        )
    if not re.fullmatch(r"compat_[a-z0-9_]+", schema):
        raise SystemExit(f"非法 schema 名 {schema!r}：只接受 ^compat_[a-z0-9_]+$")


def needs_drop_token(case: dict) -> str | None:
    """`logout_after_ward_401` 这类：先把该角色的 token 真删掉再请求，才该拿到 401。

    录制器用 `drop_token` 实现，但这个副作用没有落进夹具条目里，只能按语义还原：
    「带 Bearer 的登出 + 期望 401 登录凭证无效」⇒ 请求前该 token 必须不存在。
    """
    if case["headers"]["auth"] != "bearer":
        return None
    if case["expected_status"] != 401:
        return None
    if not case["path"].rstrip("/").endswith("/logout"):
        return None
    body = case.get("expected_body") or {}
    if body.get("message") != "登录凭证无效":
        return None
    return case["headers"]["role"]


# --------------------------------------------------------------------------------------
# 单条回放
# --------------------------------------------------------------------------------------


def replay_case(case: dict, base_url: str, session, tokens: TokenStore, max_diff: int) -> dict:
    record = {
        "name": case["name"],
        "kind": case.get("kind"),
        "method": case["method"],
        "path": case["path"],
        "path_template": case.get("path_template"),
        "role": case["headers"]["role"],
        "auth": case["headers"]["auth"],
        "expected_status": case["expected_status"],
        "expected_status_design": case.get("expected_status_design"),
        "shape_only": bool(case.get("shape_only")),
        "note": case.get("note", ""),
        "pre": [],
        "diffs": [],
        "shape_problems": [],
        "offset_style_drift": [],
    }

    drop_key = needs_drop_token(case)
    if drop_key:
        tokens.drop(drop_key)
        record["pre"].append(f"drop_token:{drop_key}")

    url, headers, body = build_request(case, base_url)
    try:
        if case.get("upload"):
            spec = case["upload"]
            png = make_png_8x8()
            if hashlib.sha256(png).hexdigest() != spec["sha256"] or len(png) != spec["size"]:
                raise SystemExit("生成的 PNG 与夹具指纹不符，make_png_8x8 需要重新核对")
            resp = session.request(
                case["method"], url, headers=headers,
                files={spec["field"]: (spec["filename"], png, spec["content_type"])},
                timeout=30,
            )
        elif case["method"] in ("POST", "PATCH", "PUT"):
            resp = session.request(
                case["method"], url, headers=headers, json=body if body is not None else {},
                timeout=30,
            )
        else:
            resp = session.request(case["method"], url, headers=headers, timeout=30)
    except Exception as exc:  # noqa: BLE001
        record["verdict"] = "transport_error"
        record["diffs"] = [f"请求失败：{type(exc).__name__}: {exc}"]
        record["actual_status"] = None
        return record

    record["actual_status"] = resp.status_code
    content_type = resp.headers.get("Content-Type", "")
    record["actual_content_type"] = content_type

    try:
        actual_body = resp.json()
    except Exception:  # noqa: BLE001
        actual_body = None
    record["actual_body_sha256"] = hashlib.sha256(resp.content).hexdigest()
    record["actual_body_bytes"] = len(resp.content)

    diffs: list[str] = []
    drift: list[str] = []

    if resp.status_code != case["expected_status"]:
        diffs.append(f"HTTP 状态：期望 {case['expected_status']}，实际 {resp.status_code}")

    expected_ct = case.get("expected_content_type") or ""
    if expected_ct and content_type.split(";")[0] != expected_ct.split(";")[0]:
        diffs.append(f"Content-Type：期望 {expected_ct}，实际 {content_type}")

    expected_body = case.get("expected_body")
    if case.get("shape_only"):
        if actual_body is None:
            diffs.append("期望 JSON，实际不是 JSON（HTML/空）")
        else:
            record["shape_problems"] = check_shape(actual_body, case.get("shape") or [])
            diffs.extend(record["shape_problems"])
    elif expected_body is not None:
        if actual_body is None:
            diffs.append(f"期望 JSON，实际不是 JSON：{short(resp.text, 200)}")
        else:
            exp_masked = apply_normalize(expected_body, case.get("normalize"))
            act_masked = apply_normalize(actual_body, case.get("normalize"))
            compare(exp_masked, act_masked, "$", diffs, drift, max_diff)

    record["diffs"] = diffs[: max_diff + 4]
    record["offset_style_drift"] = drift

    if case.get("server_error") or case["expected_status"] >= 500:
        # DEVIATIONS.md D1/D2/D5：蓝本缺陷，已裁定 Rust 侧修正 -> 不计失败
        record["verdict"] = "expected_deviation"
    elif diffs:
        record["verdict"] = "fail"
    else:
        record["verdict"] = "pass"
    return record


# --------------------------------------------------------------------------------------
# self-check：用变异测试证明比对器真的能发现差异
# --------------------------------------------------------------------------------------


def self_check(domains: dict[str, dict], max_diff: int) -> int:
    total = mutants = detected = undetected = 0
    failures: list[str] = []

    for domain in DOMAIN_ORDER:
        payload = domains.get(domain)
        if not payload:
            continue
        for case in payload["cases"]:
            if case.get("shape_only") or case.get("server_error"):
                continue
            expected = case.get("expected_body")
            if expected is None:
                continue
            total += 1

            exp_masked = apply_normalize(expected, case.get("normalize"))
            act_masked = apply_normalize(copy.deepcopy(expected), case.get("normalize"))
            diffs, drift = compare(exp_masked, act_masked, "$", [], [], max_diff)
            if diffs:
                failures.append(f"自比对居然不通过：{domain}/{case['name']} -> {diffs[0]}")

            # 找一个没被 normalize 屏蔽的叶子，改掉它，比对器必须报出差异
            if not isinstance(expected, dict):
                continue
            leaf_path = find_mutable_path(exp_masked)
            if leaf_path is None:
                continue
            mutants += 1
            broken = copy.deepcopy(exp_masked)
            if not set_path(broken, leaf_path, "MUTATED"):
                continue
            diffs2, _ = compare(exp_masked, broken, "$", [], [], max_diff)
            if any(leaf_path.rsplit(".", 1)[-1] in d or leaf_path in d for d in diffs2):
                detected += 1
            else:
                undetected += 1
                failures.append(f"变异未被发现：{domain}/{case['name']} @ {leaf_path}")

    print("=== self-check（比对器自证） ===")
    print(f"  可比对用例      {total}")
    print(f"  注入变异        {mutants}")
    print(f"  变异被发现      {detected}")
    print(f"  变异漏检        {undetected}")
    if failures:
        print("\n!! 失败项：")
        for item in failures[:20]:
            print("   ", item)
        return 1
    print("\n[ok] 比对器自比对全通过，且能发现注入的变异")
    return 0


def find_mutable_path(node, path="$") -> str | None:
    if isinstance(node, dict):
        for key in sorted(node):
            found = find_mutable_path(node[key], f"{path}.{key}")
            if found:
                return found
    elif isinstance(node, list):
        for i, item in enumerate(node):
            found = find_mutable_path(item, f"{path}[{i}]")
            if found:
                return found
    else:
        if isinstance(node, str) and PLACEHOLDER.match(node):
            return None
        return path
    return None


def set_path(node, path: str, value) -> bool:
    segs = re.findall(r"\.([A-Za-z0-9_]+)|\[(\d+)\]", path.lstrip("$"))
    cur = node
    for i, (name, index) in enumerate(segs):
        last = i == len(segs) - 1
        if name:
            if not isinstance(cur, dict) or name not in cur:
                return False
            if last:
                cur[name] = value
                return True
            cur = cur[name]
        else:
            idx = int(index)
            if not isinstance(cur, list) or idx >= len(cur):
                return False
            if last:
                cur[idx] = value
                return True
            cur = cur[idx]
    return False


# --------------------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description="契约回放与比对")
    ap.add_argument("--base-url", default="http://127.0.0.1:11000/compat")
    ap.add_argument("--fixtures", default=str(FIXTURES))
    ap.add_argument("--domain", action="append", default=None,
                    help="只回放这些域（可重复；默认全部）")
    ap.add_argument("--case", action="append", default=None, help="只回放这些用例名")
    ap.add_argument("--schema", default=None,
                    help="scratch schema（用于回放中重建固定 token）；不给则不碰数据库")
    ap.add_argument("--out", default=None, help="机器可读报告路径")
    ap.add_argument("--max-diff", type=int, default=12, help="每条用例最多报几个差异")
    ap.add_argument("--list", action="store_true", help="只列出用例并退出")
    ap.add_argument("--self-check", action="store_true",
                    help="只验证比对器自身（变异测试），不连服务、不碰数据库")
    ap.add_argument("--strict", action="store_true",
                    help="把 fixture 自身的缺陷（如末态 seed）也当失败")
    args = ap.parse_args()

    fixtures = Path(args.fixtures)
    index, domains = load_fixtures(fixtures)

    if args.self_check:
        return self_check(domains, args.max_diff)

    if args.list:
        for domain in DOMAIN_ORDER:
            for case in domains.get(domain, {}).get("cases", []):
                print(f"{domain}\t{case['kind']}\t{case['name']}\t{case['method']} {case['path']}")
        return 0

    warnings: list[str] = []

    phase = seed_phase(index, fixtures)
    if phase["phase"] == "post-replay":
        warnings.append(
            "seed.json 是**回放后的末态**（证据：" + "; ".join(phase["evidence"][:4]) + "）。"
            "而夹具设计是「加载 seed → 按序回放」，因此依赖初始状态的用例（尤其是读类的"
            "stock/sold_quantity/is_open 等）会自然不匹配。需要夹具负责人把 seed 改成"
            "**回放前**导出（capture_contract.py 里 dump_seed 目前排在 run_capture 之后）。"
        )
    if phase.get("expected_signature") and phase.get("signature") != phase["expected_signature"]:
        warnings.append(
            f"seed.json 签名与 index.seed_signature 不一致："
            f"{phase.get('signature')} != {phase['expected_signature']}"
        )
    warnings.extend(mutation_order_crosscheck(index, domains))

    selected_domains = [d for d in DOMAIN_ORDER if (not args.domain or d in args.domain)]
    cases: list[tuple[str, dict]] = []
    for domain in selected_domains:
        for case in domains.get(domain, {}).get("cases", []):
            if args.case and case["name"] not in args.case:
                continue
            cases.append((domain, case))

    tokens = TokenStore(schema_dsn(base_dsn(), args.schema) if args.schema else None, args.schema)

    # 录制器在跑**任何**用例之前就补建了 2 个 scratch 账号并装了 5 条固定 token
    # （`run_capture` 的 ensure_tokens()）。seed.json 是那之前的纯种子态，
    # 所以这里要复刻一次，否则读类用例（如 me_ok_*）从一开始就 401。
    if tokens.enabled:
        tokens.ensure_scratch_users()
        tokens.install_all()
        print(f"[tokens] 初始状态就绪：scratch 账号 2 个 + 固定 token {len(TOKEN_KEYS)} 条")

    import requests

    session = requests.Session()
    records: list[dict] = []
    for domain, case in cases:
        # 复刻录制器的 ensure_tokens()：每个写类用例前重建 5 个固定 token。
        if case.get("kind") == "mutation" and tokens.enabled:
            tokens.install_all()
        record = replay_case(case, args.base_url, session, tokens, args.max_diff)
        record["domain"] = domain
        records.append(record)
        mark = {"pass": "  ", "fail": "!!", "expected_deviation": "~~", "transport_error": "!!"}[
            record["verdict"]
        ]
        print(
            f"{mark}[{domain}] {record['name']:<48} {record['method']:<7} "
            f"{str(record['expected_status']):<5}-> {str(record['actual_status']):<5} "
            f"{record['verdict']}"
        )
        for diff in record["diffs"][:6]:
            print(f"      {diff}")

    summary = {
        "total": len(records),
        "pass": sum(1 for r in records if r["verdict"] == "pass"),
        "fail": sum(1 for r in records if r["verdict"] == "fail"),
        "expected_deviation": sum(1 for r in records if r["verdict"] == "expected_deviation"),
        "transport_error": sum(1 for r in records if r["verdict"] == "transport_error"),
        "shape_only": sum(1 for r in records if r["shape_only"]),
        "offset_style_drift": sum(len(r["offset_style_drift"]) for r in records),
    }
    by_domain = {}
    for domain in selected_domains:
        subset = [r for r in records if r["domain"] == domain]
        if not subset:
            continue
        by_domain[domain] = {
            "total": len(subset),
            "pass": sum(1 for r in subset if r["verdict"] == "pass"),
            "fail": sum(1 for r in subset if r["verdict"] == "fail"),
            "expected_deviation": sum(1 for r in subset if r["verdict"] == "expected_deviation"),
        }

    print("\n=== 汇总 ===")
    print(
        f"总计 {summary['total']}  通过 {summary['pass']}  失败 {summary['fail']}  "
        f"预期偏差 {summary['expected_deviation']}  传输错误 {summary['transport_error']}  "
        f"shape_only {summary['shape_only']}  tz 漂移 {summary['offset_style_drift']}"
    )
    for domain, stat in by_domain.items():
        print(f"  {domain:<14} {stat}")

    if summary["offset_style_drift"]:
        print("\n[tz] `Z` 与 `+00:00` 混用（语义等价，仅计数）：")
        for record in records:
            for path in record["offset_style_drift"][:3]:
                print(f"   [{record['domain']}] {record['name']} {path}")

    if warnings:
        print("\n!! fixture 告警：")
        for warn in warnings:
            print("   -", warn)

    report = {
        "base_url": args.base_url,
        "schema": args.schema,
        "fixtures": str(fixtures),
        "seed_phase": phase,
        "warnings": warnings,
        "summary": summary,
        "by_domain": by_domain,
        "cases": records,
    }
    if args.out:
        Path(args.out).write_text(
            json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(f"\n[out] {args.out}")

    if summary["fail"] or summary["transport_error"]:
        return 1
    if args.strict and warnings:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
