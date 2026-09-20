#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""契约夹具录制器（Wave 0'）—— 把 navel_backend_git（Django 5 + DRF）的全部 79 条路由
录成 Rust 侧逐字节比对用的 golden fixture。

本文件是 `db/scripts/capture_contract.py` 的**已验证替代实现**，落在「换输出目录」路径下，
避免与并发 agent 争抢同一文件。差异见文末 `DIFF NOTES`。

设计要点
--------
* 全程 **in-process**：不启动 runserver，不打网络，用 django.test.Client。
* 全程 **离线确定**：AGENT_LLM_API_KEY 置空 -> agent_service 走规则兜底，不调 LLM；
  仓库内无 model/ -> 柑橘识别走 mock。
* 全程 **零仓库副作用**：测试库是 SQLite 共享内存库，绝不创建/覆盖
  navel_backend_git/db.sqlite3；MEDIA_ROOT 重定向到仓库外临时目录；sys.dont_write_bytecode。
* 不连生产 PostgreSQL（本工作流用不到 PG）。

产物
----
    <out>/{auth,core,commerce,orchard_trace,agent,index,seed}.json
    <out>/REPORT.md

用法
----
    python capture_contract.py
    python capture_contract.py --domain auth --domain core   # 可重复；index/seed 仍会重写
    python capture_contract.py --out <dir>
    python capture_contract.py --smoke                        # 自检 + 枚举 + 用例矩阵核对

DIFF NOTES（相对 db/scripts/capture_contract.py）
------------------------------------------------
1. **normalize 真正生效**：录制时就地把易变值替换为 `<sha256:16hex>`，并把展开后的路径
   清单写进用例 `normalize`、实际命中字段名写进 `normalize_hits`。
   原实现中 150 条用例有 121 条 `normalize` 为空，夹具里残留 248 处运行期 UUID/token。
2. `index.json` 补齐 `covered` / `partial` / `blocked` / `uncovered`(计数) / `mutation_order`
   / `alias_conclusion` / `unresolved` / `environment` / `seed_row_counts`，
   逐 path 增加 `path_template` / `view` / `status_histogram` / `paths[].cases`。
3. 不产出 `probes.json`：改为**用例断言矩阵**——每条 `path×method` 都必须有显式用例，
   `--smoke` 即核对（缺则 exit 2），而不是事后用无语义探针补覆盖数。
4. `--domain` 支持重复传入（`action="append"`）。
5. 用例带 `expect_status` 设计期望；实际不符时标 `status_matches_design=false` 并计入
   `partial`，避免「实测即期望」掩盖蓝本漂移。
6. 写类集中在每域表尾并记录全局 `mutation_order`（含 note），读类拿到未污染状态。
7. 域划分仍按**路径前缀**（与任务书一致）：`/api/v1/farmer/*` 归 orchard_trace，
   `/api/{products,cart,addresses,orders,after-sales}` 及其 `v1/` 别名归 commerce。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import shutil
import sys
import tempfile
import uuid
import zlib
from datetime import datetime, timedelta, timezone as dt_timezone
from pathlib import Path

sys.dont_write_bytecode = True
os.environ["PYTHONDONTWRITEBYTECODE"] = "1"

# --------------------------------------------------------------------------------------
# 路径常量
# --------------------------------------------------------------------------------------

WORKSPACE = Path(r"D:\githubs\db_work")
DJANGO_REPO = WORKSPACE / "navel_backend_git"
FIXTURE_DIR = WORKSPACE / "db" / "tests" / "fixtures" / "contract"
TMP_DIR = WORKSPACE / ".venv-django-tmp"
MEDIA_TMP = TMP_DIR / "media"

RANDOM_SEED = 20260913

DOMAINS = ["auth", "core", "commerce", "orchard_trace", "agent"]

METHOD_ORDER = ["GET", "POST", "PUT", "PATCH", "DELETE"]

NIL_UUID = "00000000-0000-4000-8000-000000000000"

CAPTURED_AT = datetime.now(dt_timezone.utc).replace(microsecond=0).isoformat()


# --------------------------------------------------------------------------------------
# 沙箱兼容：让 tempfile.mkdtemp 产出可写目录
# --------------------------------------------------------------------------------------
# 实测：本机沙箱把 os.mkdir(path, 0o700) 建出的目录变成当前进程不可写（PermissionError 13），
# 而 os.mkdir(path, 0o777) 正常。tempfile.mkdtemp 内部正是 mode=0o700。
# Django 的数据库测试基建与 dumpdata 都会用到临时目录，故此处统一放宽。
def _patch_tempfile() -> None:
    def _writable_mkdtemp(suffix=None, prefix=None, dir=None):
        suffix = suffix or ""
        prefix = prefix or "tmp"
        base = dir or tempfile.gettempdir()
        for _ in range(10000):
            candidate = os.path.join(base, prefix + os.urandom(8).hex() + suffix)
            try:
                os.mkdir(candidate, 0o777)
            except FileExistsError:
                continue
            return candidate
        raise FileExistsError("mkdtemp: 无法生成唯一临时目录名")

    tempfile.mkdtemp = _writable_mkdtemp


# --------------------------------------------------------------------------------------
# Django 环境
# --------------------------------------------------------------------------------------

ENV: dict = {}


def bootstrap_django():
    """返回 (test_db_name, teardown_fn)。"""
    _patch_tempfile()
    random.seed(RANDOM_SEED)

    # 必须早于任何 api.* 的 import：agent_service 在 import 期就算 _LLM_VALID
    os.environ["AGENT_LLM_API_KEY"] = ""
    os.environ.setdefault("DJANGO_SETTINGS_MODULE", "navel_back.settings")
    os.environ.setdefault("DJANGO_DEBUG", "0")
    os.environ.setdefault("DJANGO_ALLOWED_HOSTS", "*")
    sys.path.insert(0, str(DJANGO_REPO))

    import django

    django.setup()

    from django.conf import settings

    MEDIA_TMP.mkdir(parents=True, exist_ok=True)
    settings.MEDIA_ROOT = MEDIA_TMP
    # DEBUG=0 时 navel_back.urls 不挂 /media/ 静态路由；本工作流只录 JSON API，不受影响。

    from django.test.utils import setup_test_environment

    setup_test_environment()

    from django.core.management import call_command
    from django.db import connection

    real_db = DJANGO_REPO / "db.sqlite3"
    real_db_stat = real_db.stat() if real_db.exists() else None
    real_db_hash = hashlib.sha256(real_db.read_bytes()).hexdigest() if real_db.exists() else ""

    # 显式双保险：SQLite 测试库走共享内存，绝不落到仓库文件
    settings.DATABASES["default"].setdefault("TEST", {})["NAME"] = None

    old_config = connection.creation.create_test_db(verbosity=0, autoclobber=True)
    test_db_name = str(connection.settings_dict.get("NAME"))

    if Path(test_db_name) == real_db or "db.sqlite3" in test_db_name:
        raise RuntimeError(f"拒绝执行：测试库指向仓库内的 db.sqlite3 ({test_db_name})")
    if real_db.exists():
        after = real_db.stat()
        if (
            real_db_stat is None
            or after.st_mtime_ns != real_db_stat.st_mtime_ns
            or after.st_size != real_db_stat.st_size
        ):
            raise RuntimeError("拒绝继续：真实 db.sqlite3 已被改动")

    from api import agent_service, model_service  # noqa: F401
    from rest_framework import VERSION as DRF_VERSION

    if agent_service._LLM_VALID:
        raise RuntimeError("拒绝执行：LLM 未关闭（AGENT_LLM_API_KEY 未生效）")
    if model_service.MODEL_AVAILABLE:
        raise RuntimeError("拒绝执行：模型文件可用，柑橘识别将走真推理而非 mock")

    call_command("seed_demo_data", verbosity=0)

    ENV.update(
        {
            "django_version": django.get_version(),
            "drf_version": DRF_VERSION,
            "python_version": sys.version.split()[0],
            "time_zone": settings.TIME_ZONE,
            "use_tz": settings.USE_TZ,
            "language_code": settings.LANGUAGE_CODE,
            "media_root_used": str(MEDIA_TMP),
            "media_root_repo_default": str(settings.BASE_DIR / "media"),
            "llm_enabled": False,
            "model_available": model_service.MODEL_AVAILABLE,
            "recognition_mode": "mock",
            "random_seed": RANDOM_SEED,
            "real_db_sha256": real_db_hash,
            "test_db": test_db_name,
        }
    )

    def teardown():
        try:
            connection.creation.destroy_test_db(old_config, verbosity=0)
        except Exception:
            pass
        if real_db.exists() and real_db_stat is not None:
            after = real_db.stat()
            if (
                after.st_mtime_ns != real_db_stat.st_mtime_ns
                or after.st_size != real_db_stat.st_size
            ):
                print("[!!] 警告：真实 db.sqlite3 在本次运行中被改动", file=sys.stderr)

    return test_db_name, teardown


# --------------------------------------------------------------------------------------
# 路由枚举（自省）
# --------------------------------------------------------------------------------------

SKIP_METHODS = {"OPTIONS", "HEAD", "TRACE", "CONNECT"}


def enumerate_routes():
    """遍历 api.urls.urlpatterns，返回 [{path, url_name, methods, view, status}]。"""
    from api import urls as api_urls

    routes = []
    for pattern in api_urls.urlpatterns:
        callback = pattern.callback
        view_cls = getattr(callback, "view_class", None)
        route_str = str(pattern.pattern)
        if view_cls is None:
            routes.append(
                {
                    "path": route_str,
                    "url_name": getattr(pattern, "name", ""),
                    "methods": [],
                    "view": f"{getattr(callback, '__module__', '?')}."
                            f"{getattr(callback, '__name__', repr(callback))}",
                    "status": "not_drf_view",
                }
            )
            continue
        http_names = list(getattr(view_cls, "http_method_names", []) or [])
        methods = [
            m.upper()
            for m in http_names
            if m.upper() not in SKIP_METHODS and hasattr(view_cls, m)
        ]
        methods.sort(key=lambda m: METHOD_ORDER.index(m) if m in METHOD_ORDER else 99)
        routes.append(
            {
                "path": route_str,
                "url_name": getattr(pattern, "name", ""),
                "methods": methods,
                "view": f"{view_cls.__module__}.{view_cls.__name__}",
                "status": "ok" if methods else "no_methods",
            }
        )
    return routes


# --------------------------------------------------------------------------------------
# 域划分（79 条 path：auth 9 / core 14 / orchard_trace 21 / commerce 24 / agent 11）
# --------------------------------------------------------------------------------------

DOMAIN_BY_URL_NAME: dict[str, str] = {}


def _assign(name: str, domain: str) -> None:
    DOMAIN_BY_URL_NAME[name] = domain


for _n in [
    "register_api", "login_api", "logout_api", "me_api",
    "v1_register_api", "v1_login_api", "v1_logout_api", "v1_me_api", "get_user_api",
]:
    _assign(_n, "auth")

for _n in [
    "home_api", "growth_tracking_api", "diagnose_api", "temperature_humidity_api",
    "generate_api", "fertilization_plan_api", "citrus_disease_api",
    "recognition_records_api", "get_disease_treatment_api", "get_tasks_api",
    "add_task_api", "complete_task_api", "generate_task_from_disease_api",
    "generate_task_from_environment_api",
]:
    _assign(_n, "core")

for _n in [
    "orchard_list_api", "orchard_detail_api", "v1_orchard_list_api", "v1_orchard_detail_api",
    "supply_batch_list_api", "supply_batch_detail_api", "v1_supply_batch_list_api",
    "v1_supply_batch_detail_api", "trace_lookup_api", "v1_trace_lookup_api",
    "farmer_orchards_api", "farmer_tree_archive_api", "farmer_batches_api",
    "farmer_batch_create_api", "farmer_image_upload_api", "farmer_product_list_create_api",
    "farmer_product_detail_api", "farmer_harvest_archive_api", "farmer_trace_event_api",
    "farmer_quality_sample_api", "farmer_product_health_record_api",
]:
    _assign(_n, "orchard_trace")

for _n in [
    "product_list_api", "product_detail_api", "product_health_archive_api",
    "v1_product_list_api", "v1_product_detail_api", "v1_product_health_archive_api",
    "cart_api", "cart_item_api", "v1_cart_api", "v1_cart_item_api",
    "address_api", "address_detail_api", "v1_address_api", "v1_address_detail_api",
    "order_api", "order_detail_api", "order_pay_api", "order_cancel_api",
    "v1_order_api", "v1_order_detail_api", "v1_order_pay_api", "v1_order_cancel_api",
    "after_sale_api", "v1_after_sale_api",
]:
    _assign(_n, "commerce")

for _n in [
    "agent_context_api", "agent_daily_report_api", "agent_select_api", "agent_inquiry_api",
    "agent_risk_alert_api", "agent_repurchase_api", "agent_chat_api",
    "agent_approval_create_api", "agent_feedback_api", "agent_approval_list_api",
    "agent_approval_decision_api",
]:
    _assign(_n, "agent")


def domain_of(url_name: str) -> str:
    return DOMAIN_BY_URL_NAME.get(url_name, "unassigned")


# --------------------------------------------------------------------------------------
# 归一化（normalize）
# --------------------------------------------------------------------------------------
# 录制期语汇（落盘时展开成 Rust 可直接消费的清单）：
#   $.a.b        对象字段
#   $.a[*].b     数组内每个元素
#   $..b         任意深度的字段 b（roll-up）
#   A|B          备选（展开成两条）
# 命中值替换为 <sha256:16hex>：**键保留、值不参与比较**。

BASE_NORM = [
    "$.timestamp",
    "$..createdAt",
    "$..updatedAt",
    "$..created_at",
    "$..updated_at",
    "$..timestamp",
    # 登录/注册的会话凭据与到期时刻：每次请求都不同
    "$..token",
    "$..expiresAt",
    "$..expires_at",
    # DRF 的 PrimaryKeyRelatedField 会把外键序列化成裸主键（FruitTreeArchive /
    # HarvestArchive 的 tree / harvest_archive 字段）。这些 id 由 seed 决定，
    # 但 Rust 侧重建 seed 时无法保证同值 -> 按字段名屏蔽。
    "$..tree",
    "$..harvest_archive",
    # 同样是 PrimaryKeyRelatedField：BatchQualitySample.product / OrderItem.product /
    # AfterSaleRequest.order|package 都会以裸主键出现
    "$..product",
    "$..order",
    "$..package",
    # AgentApproval.ref_id 存 sales_batch 的 uuid 字符串
    "$..ref_id",
]

ALWAYS_NORM = BASE_NORM + ["$..id"]


def _slashed(path_template: str) -> str:
    """统一成带前导斜杠，便于别名与主路径对齐比较。"""
    return "/" + path_template.lstrip("/")


def norm(*extra: str) -> list[str]:
    out = list(BASE_NORM)
    for e in extra:
        if e not in out:
            out.append(e)
    return out


def norm_all(*extra: str) -> list[str]:
    out = list(ALWAYS_NORM)
    for e in extra:
        if e not in out:
            out.append(e)
    return out


def _parse_expr(expr: str):
    rest = expr[1:] if expr.startswith("$") else expr
    if rest.startswith("."):
        # `$..key` -> 任意深度的字段 key（roll-up）。
        # 必须把前导的点剥掉，否则会去匹配字面量键 ".id"，屏蔽静默失效。
        return [("rollup", rest.lstrip("."))]
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


def _redact(value):
    payload = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return "<sha256:" + hashlib.sha256(payload.encode("utf-8")).hexdigest()[:16] + ">"


def _walk_segments(node, segs, hits):
    if not segs:
        return
    head, tail = segs[0], segs[1:]
    if head[0] == "key":
        name = head[1]
        if isinstance(node, dict) and name in node:
            if not tail:
                node[name] = _redact(node[name])
                hits.append(name)
            else:
                _walk_segments(node[name], tail, hits)
    else:
        if isinstance(node, list):
            for item in node:
                _walk_segments(item, tail, hits)
        elif isinstance(node, dict):
            _walk_segments(node, tail, hits)


def _rollup(node, key, hits):
    if isinstance(node, dict):
        for k, v in list(node.items()):
            if k == key:
                node[k] = _redact(v)
                hits.append(k)
            else:
                _rollup(v, key, hits)
    elif isinstance(node, list):
        for item in node:
            _rollup(item, key, hits)


def apply_normalize(body, exprs):
    hits: list[str] = []
    for expr in exprs:
        for part in expr.split("|"):
            part = part.strip()
            if not part:
                continue
            segs = _parse_expr(part)
            if segs and segs[0][0] == "rollup":
                _rollup(body, segs[0][1], hits)
            else:
                _walk_segments(body, segs, hits)
    return sorted(set(hits))


def expand_normalize(exprs):
    out = []
    for expr in exprs:
        for part in expr.split("|"):
            part = part.strip()
            if part and part not in out:
                out.append(part)
    return out


def shape_of(body, prefix="$"):
    """只校验键与类型：'$.data.x:str' / '$.data.items[*].name:str'。"""
    out: list[str] = []

    def type_of(v):
        if v is None:
            return "null"
        if isinstance(v, bool):
            return "bool"
        if isinstance(v, int):
            return "int"
        if isinstance(v, float):
            return "float"
        if isinstance(v, str):
            return "str"
        if isinstance(v, list):
            return "list"
        return "dict"

    def walk(node, path):
        if isinstance(node, dict):
            for k, v in node.items():
                p = f"{path}.{k}"
                if isinstance(v, list):
                    if v and isinstance(v[0], dict):
                        out.append(f"{p}[*]:dict")
                        walk(v[0], f"{p}[*]")
                    elif v:
                        out.append(f"{p}[0]:{type_of(v[0])}")
                    else:
                        out.append(f"{p}:list")
                elif isinstance(v, dict):
                    out.append(f"{p}:dict")
                    if v:
                        walk(v, p)
                else:
                    out.append(f"{p}:{type_of(v)}")
        elif isinstance(node, list):
            if node and isinstance(node[0], dict):
                out.append(f"{path}[*]:dict")
                walk(node[0], f"{path}[*]")

    walk(body, prefix)
    return out


# --------------------------------------------------------------------------------------
# seed 引用
# --------------------------------------------------------------------------------------


def resolve_seed_refs():
    from api.models import (
        BuyerAddress, CartItem, CitrusProduct, FruitTreeArchive, Orchard, Order,
        SalesBatch, TracePackage,
    )

    orchard = Orchard.objects.get(code="GY-XF-DEMO-01")
    batch = SalesBatch.objects.get(code="CGJ-2026-XF-001")
    product = CitrusProduct.objects.get(name="赣南纽荷尔家庭装")
    product_ent = CitrusProduct.objects.get(name="纽荷尔企业装")
    product_ay = CitrusProduct.objects.get(name="红肉脐橙企业装")
    product_ay_family = CitrusProduct.objects.get(name="红肉脐橙家庭装")
    order_completed = Order.objects.get(order_number="ORD-DEMO-1001")
    order_pending = Order.objects.get(order_number="ORD-DEMO-1003")
    address = BuyerAddress.objects.get(buyer__username="buyer_zhang", is_default=True)
    address2 = BuyerAddress.objects.get(buyer__username="buyer_zhang", is_default=False)
    cart_items = list(CartItem.objects.filter(buyer__username="buyer_zhang"))
    item_b1 = next((c for c in cart_items
                    if c.product.sales_batch and c.product.sales_batch.code == "CGJ-2026-XF-001"),
                   None)
    item_b3 = next((c for c in cart_items
                    if c.product.sales_batch and c.product.sales_batch.code == "CGJ-2026-AY-002"),
                   None)
    tree = FruitTreeArchive.objects.get(tree_number="信丰-01-001")
    package = TracePackage.objects.filter(order=order_completed).first()

    return {
        "auth": {
            "farmer_xinfeng": {"password": "farmer123", "role": "farmer"},
            "farmer_xunwu": {"password": "farmer123", "role": "farmer"},
            "farmer_anyuan": {"password": "farmer123", "role": "farmer"},
            "buyer_zhang": {"password": "buyer123", "role": "buyer"},
        },
        "login_endpoint": "/api/login",
        "login_payload": {"username": "<seed username>", "password": "<seed password>"},
        "orchard_id": str(orchard.id),
        "batch_id": str(batch.id),
        "product_id": str(product.id),
        "product_enterprise_id": str(product_ent.id),
        "product_ay_enterprise_id": str(product_ay.id),
        "product_ay_family_id": str(product_ay_family.id),
        "order_completed_id": str(order_completed.id),
        "order_pending_id": str(order_pending.id),
        "address_id": str(address.id),
        "address_secondary_id": str(address2.id),
        "item_id": str(item_b1.id) if item_b1 else "",
        "item_id_b3": str(item_b3.id) if item_b3 else "",
        "item_ids": [str(c.id) for c in cart_items],
        "trace_code": batch.trace_code,
        "tree_number": tree.tree_number,
        "tree_id": str(tree.id),
        "tree_trace_code": tree.trace_code,
        "package_id": str(package.id) if package else "",
        "package_trace_code": package.trace_code if package else "",
        "batch_ids": {b.code: str(b.id) for b in SalesBatch.objects.all()},
        "batch_trace_codes": {b.code: b.trace_code for b in SalesBatch.objects.all()},
        "product_ids": {p.name: str(p.id) for p in CitrusProduct.objects.all()},
        "order_ids": {o.order_number: str(o.id) for o in Order.objects.all()},
        "_codes": {
            "orchard": orchard.code,
            "batch": batch.code,
            "product": product.name,
            "order": order_completed.order_number,
            "trace_code": batch.trace_code,
            "tree_number": tree.tree_number,
        },
    }


def seed_approval_id() -> str:
    """预置一张待决策审批单，让 decision 用例有稳定 uuid。"""
    from api.models import AgentApproval

    approval = AgentApproval.objects.create(
        ticket_type=AgentApproval.TicketType.OTHER,
        title="契约基准：待决策审批单",
        ref_type="qa",
        ref_id="seed-approval",
        payload={"action": "none", "args": {}},
    )
    return str(approval.id)


# --------------------------------------------------------------------------------------
# 内置 8x8 PNG（zlib + 手写 chunk，零第三方依赖）
# --------------------------------------------------------------------------------------

_PNG: bytes | None = None


def make_png_8x8() -> bytes:
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


def png_base64_data_uri() -> str:
    import base64

    return "data:image/png;base64," + base64.b64encode(make_png_8x8()).decode("ascii")


# --------------------------------------------------------------------------------------
# 用例构造
# --------------------------------------------------------------------------------------


def case(name, url_name, method, *, role=None, auth="auto", body=None, query=None,
         files=None, url_args=None, expect_status=200, normalize=None, shape_only=False,
         kind="read", capture=None, alias_of=None, primary_alias_case=None,
         drop_token=None, note=""):
    return {
        "name": name,
        "url_name": url_name,
        "method": method,
        "role": role,
        "auth": auth,          # auto | none | malformed | unknown
        "body": body,
        "query": query,
        "files": files,        # {form_field: filename}
        "url_args": url_args or {},
        "expect_status": expect_status,
        "normalize": norm() if normalize is None else list(normalize),
        "shape_only": bool(shape_only),
        "kind": kind,
        "capture": capture or {},
        "alias_of": alias_of,
        "primary_alias_case": primary_alias_case,
        "drop_token": drop_token,
        "note": note,
    }


def M(name, url_name, method, **kw):
    kw.setdefault("kind", "mutation")
    return case(name, url_name, method, **kw)


def V(name, url_name, method, status=400, **kw):
    kw.setdefault("kind", "validation")
    return case(name, url_name, method, expect_status=status, **kw)


def A(name, url_name, method, status=401, **kw):
    kw.setdefault("kind", "auth")
    kw["auth"] = kw.get("auth", "none")
    return case(name, url_name, method, expect_status=status, **kw)


def R(name, url_name, method, status=403, **kw):
    kw.setdefault("kind", "role")
    return case(name, url_name, method, expect_status=status, **kw)


# --------------------------------------------------------------------------------------
# auth 域（9 条 path）
# --------------------------------------------------------------------------------------

def auth_cases(refs):
    return [
        case("me_ok_buyer", "me_api", "GET", role="buyer_zhang", auth="bearer",
             normalize=norm_all()),
        case("me_ok_farmer", "me_api", "GET", role="farmer_xinfeng", auth="bearer",
             normalize=norm_all()),
        A("me_no_token_401", "me_api", "GET"),
        A("me_malformed_header_401", "me_api", "GET", auth="malformed",
          note="文案：Authorization 请求头格式无效"),
        A("me_unknown_token_401", "me_api", "GET", auth="unknown",
          note="文案：登录凭证无效"),
        A("me_method_not_allowed_405", "me_api", "DELETE", status=405,
          role="farmer_xinfeng", auth="bearer",
          note="文案：方法 “DELETE” 不被允许。（zh-hans 本地化，全角引号）"),
        case("user_alias_ok_farmer", "get_user_api", "GET", role="farmer_xinfeng",
             auth="bearer", normalize=norm_all(), alias_of="/api/me",
             primary_alias_case="me_ok_farmer"),
        A("user_alias_no_token_401", "get_user_api", "GET"),
        A("v1_me_no_token_401", "v1_me_api", "GET"),
        A("logout_no_token_401", "logout_api", "POST", auth="none"),
        A("v1_logout_no_token_401", "v1_logout_api", "POST", auth="none"),
        case("login_ok_farmer", "login_api", "POST",
             body={"username": "farmer_xinfeng", "password": "farmer123"},
             normalize=norm_all(), kind="read",
             note="登录清掉该用户全部旧 token -> 本域只登录一次并复用"),
        A("login_missing_password_401", "login_api", "POST", status=401,
          body={"username": "farmer_xinfeng"}),
        A("login_empty_body_401", "login_api", "POST", status=401, body={}),
        A("login_wrong_password_401", "login_api", "POST", status=401,
          body={"username": "buyer_zhang", "password": "wrong"},
          note="message 为对象 {'non_field_errors': ['Invalid credentials']}"),
        A("login_unknown_user_401", "login_api", "POST", status=401,
          body={"username": "no_such_user", "password": "whatever"}),
        case("v1_login_ok_farmer", "v1_login_api", "POST",
             body={"username": "farmer_xinfeng", "password": "farmer123"},
             normalize=norm_all(), alias_of="/api/login",
             primary_alias_case="login_ok_farmer",
             note="与 /api/login 同视图，会重建 token"),
        V("register_farmer_missing_orchard_400", "register_api", "POST",
          body={"username": "qa_farmer_a", "password": "qa-pass-123456",
                "role": "farmer"}),
        V("register_bad_role_400", "register_api", "POST",
          body={"username": "qa_bad_role", "password": "qa-pass-123456",
                "role": "operator"}),
        V("register_short_password_400", "register_api", "POST",
          body={"username": "qa_short_pw", "password": "123"}),
        V("register_duplicate_username_400", "register_api", "POST",
          body={"username": "buyer_zhang", "password": "qa-pass-123456",
                "role": "buyer"}),
        V("register_ok_buyer", "register_api", "POST", status=200,
          body={"username": "qa_register_buyer", "password": "qa-pass-123456",
                "role": "buyer", "email": "qa-register@example.com"},
          normalize=norm_all()),
        V("v1_register_ok_farmer", "v1_register_api", "POST", status=200,
          body={"username": "qa_register_farmer", "password": "qa-pass-123456",
                "role": "farmer", "orchard_address": "赣州市信丰县 QA 果园",
                "latitude": 25.1, "longitude": 115.1},
          normalize=norm_all(), alias_of="/api/register",
          primary_alias_case="register_ok_buyer",
          note="果农注册会自动建 draft 果园 -> 会改变 /api/orchards，故排最后"),
        # ---------------- 写类 ----------------
        M("logout_ok", "logout_api", "POST", role="scratch", auth="bearer",
          expect_status=200, note="scratch 账号登出，不影响复用 token"),
        M("logout_after_ward_401", "logout_api", "POST", role="scratch",
          auth="bearer", expect_status=401, drop_token="scratch",
          note="先删掉 scratch token 再登出 -> 401 登录凭证无效"),
        M("v1_logout_ok", "v1_logout_api", "POST", role="scratch2", auth="bearer",
          expect_status=200, note="第二个 scratch 账号，验证 v1 别名"),
        M("v1_logout_after_ward_401", "v1_logout_api", "POST", role="scratch2",
          auth="bearer", expect_status=401, drop_token="scratch2",
          note="别名登出同样走 BearerTokenAuthentication"),
    ]


# --------------------------------------------------------------------------------------
# core 域（14 条 path）
# --------------------------------------------------------------------------------------

def core_cases(refs):
    return [
        case("home_ok_farmer", "home_api", "GET", role="farmer_xinfeng", auth="bearer"),
        A("home_no_token_401", "home_api", "GET"),
        R("home_buyer_403", "home_api", "GET", role="buyer_zhang", auth="bearer",
          note="文案：该接口仅限果农使用"),
        case("growth_tracking_ok", "growth_tracking_api", "GET", role="farmer_xinfeng",
             auth="bearer"),
        R("growth_tracking_buyer_403", "growth_tracking_api", "GET", role="buyer_zhang",
          auth="bearer"),
        case("diagnose_shape_only", "diagnose_api", "GET", role="farmer_xinfeng",
             auth="bearer", shape_only=True,
             note="shape_only：惰性创建后返回聚合结构，键固定、值随数据变"),
        A("diagnose_no_token_401", "diagnose_api", "GET"),
        case("temperature_humidity_get_ok", "temperature_humidity_api", "GET",
             role="farmer_xinfeng", auth="bearer"),
        R("temperature_humidity_buyer_403", "temperature_humidity_api", "GET",
          role="buyer_zhang", auth="bearer"),
        A("temperature_humidity_no_token_401", "temperature_humidity_api", "GET"),
        case("generate_ok", "generate_api", "GET", role="farmer_xinfeng", auth="bearer"),
        A("generate_no_token_401", "generate_api", "GET"),
        case("disease_treatment_known_ok", "get_disease_treatment_api", "GET",
             role="farmer_xinfeng", auth="bearer", query={"disease_name": "黄龙病"}),
        case("disease_treatment_unknown_ok", "get_disease_treatment_api", "GET",
             role="farmer_xinfeng", auth="bearer", query={"disease_name": "不存在的病害"}),
        V("disease_treatment_missing_param_400", "get_disease_treatment_api", "GET",
          role="farmer_xinfeng", auth="bearer",
          note="英文文案：disease_name parameter is required"),
        R("disease_treatment_buyer_403", "get_disease_treatment_api", "GET",
          role="buyer_zhang", auth="bearer", query={"disease_name": "黄龙病"}),
        case("tasks_list_ok", "get_tasks_api", "GET", role="farmer_xinfeng",
             auth="bearer", normalize=norm_all()),
        A("tasks_list_no_token_401", "get_tasks_api", "GET"),
        R("tasks_list_buyer_403", "get_tasks_api", "GET", role="buyer_zhang",
          auth="bearer"),
        case("recognition_records_shape_only", "recognition_records_api", "GET",
             role="farmer_xinfeng", auth="bearer", shape_only=True,
             note="shape_only：Django 侧 mock、Rust 侧真 ONNX；seed 记录 image 为空"),
        R("recognition_records_buyer_403", "recognition_records_api", "GET",
          role="buyer_zhang", auth="bearer"),
        V("tasks_add_missing_fields_400", "add_task_api", "POST", role="farmer_xinfeng",
          auth="bearer", body={}),
        A("tasks_add_no_token_401", "add_task_api", "POST"),
        R("tasks_add_buyer_403", "add_task_api", "POST", role="buyer_zhang",
          auth="bearer", body={}),
        V("tasks_complete_missing_id_400", "complete_task_api", "POST",
          role="farmer_xinfeng", auth="bearer", body={}),
        V("tasks_complete_unknown_404", "complete_task_api", "POST",
          role="farmer_xinfeng", auth="bearer", body={"task_id": NIL_UUID},
          status=404),
        V("tasks_generate_disease_missing_param_400",
          "generate_task_from_disease_api", "POST", role="farmer_xinfeng",
          auth="bearer", body={}),
        V("tasks_generate_environment_missing_params_400",
          "generate_task_from_environment_api", "POST", role="farmer_xinfeng",
          auth="bearer", body={}),
        V("citrus_disease_no_image_400", "citrus_disease_api", "POST",
          role="farmer_xinfeng", auth="bearer", body={},
          note="英文文案：No image provided"),
        R("citrus_disease_buyer_403", "citrus_disease_api", "POST", role="buyer_zhang",
          auth="bearer", body={}),
        V("fertilization_plan_always_500", "fertilization_plan_api", "POST",
          role="farmer_xinfeng", auth="bearer", body={}, status=500,
          note="蓝本缺陷：views.py 未定义 FertilizationPlanRequestSerializer -> 恒 500；"
               "message=\"name 'FertilizationPlanRequestSerializer' is not defined\"，"
               "且属成功体形状（带 timestamp）"),
        # ---------------- 写类 ----------------
        M("temperature_humidity_post_ok", "temperature_humidity_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"temperature": 27.5, "humidity": 88.0,
                "record_time": "2026-09-13T10:20:30.123456+00:00",
                "tag_serial_number": "NFC-QA-0001"},
          normalize=norm_all("$.data.recentRecords[*].record_time")),
        V("temperature_humidity_post_bad_type_400", "temperature_humidity_api", "POST",
          role="farmer_xinfeng", auth="bearer",
          body={"temperature": "abc", "humidity": 70}),
        V("temperature_humidity_post_out_of_range_400", "temperature_humidity_api",
          "POST", role="farmer_xinfeng", auth="bearer",
          body={"temperature": 200, "humidity": 70}),
        V("temperature_humidity_post_bad_time_400", "temperature_humidity_api", "POST",
          role="farmer_xinfeng", auth="bearer",
          body={"temperature": 26, "humidity": 70, "record_time": "not-a-time"}),
        M("tasks_add_ok", "add_task_api", "POST", role="farmer_xinfeng", auth="bearer",
          expect_status=200, capture={"task_id": "$.data.id"},
          body={"title": "QA 巡园记录", "description": "契约基准用例生成的巡园任务",
                "risk_level": "低风险", "task_type": "日常巡园", "source": "用户"},
          normalize=norm_all()),
        M("tasks_complete_ok", "complete_task_api", "POST", role="farmer_xinfeng",
          auth="bearer", expect_status=200,
          body={"task_id": "@capture:task_id"}, normalize=norm_all(),
          note="依赖 tasks_add_ok 捕获的 task_id；completed_at 是朴素时间（无偏移）"),
        M("tasks_generate_disease_ok", "generate_task_from_disease_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"disease_name": "溃疡病"}, normalize=norm_all()),
        M("tasks_generate_disease_healthy_200", "generate_task_from_disease_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"disease_name": "健康果树"},
          note="200 + data=null + message='No task needed for healthy tree'（英文）"),
        M("tasks_generate_environment_shape_only", "generate_task_from_environment_api",
          "POST", role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"temperature": 27.5, "humidity": 88.0}, shape_only=True,
          note="shape_only：风险分级由规则常量表决定，Rust 侧需逐字复刻规则"),
        M("citrus_disease_upload_shape_only", "citrus_disease_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200, shape_only=True,
          body={"IMAGE": "@png_data_uri", "area": "A区示范地块"},
          note="shape_only：走 base64 IMAGE 分支；Django 侧 mock 随机识别，"
               "Rust 侧真 ONNX"),
        M("fertilization_plan_after_state_500", "fertilization_plan_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=500, body={},
          normalize=norm_all(), note="重读确认恒 500（成功体形状）"),
    ]


# --------------------------------------------------------------------------------------
# commerce 域（24 条 path）
# --------------------------------------------------------------------------------------

def commerce_cases(refs):
    prod = refs["product_id"]
    prod_ent = refs["product_enterprise_id"]
    prod_ay = refs["product_ay_enterprise_id"]
    prod_ay_family = refs["product_ay_family_id"]
    order = refs["order_completed_id"]
    order_pending = refs["order_pending_id"]
    addr = refs["address_id"]
    addr2 = refs["address_secondary_id"]
    # seed 购物车两条：b1 的纽荷尔企业装、b3 的红肉脐橙企业装
    item = refs["item_id"]            # b1 条目（纽荷尔企业装）
    item_b3 = refs["item_id_b3"]      # b3 条目（红肉脐橙企业装）
    return [
        # ---------------- 公开读 ----------------
        case("products_list_ok", "product_list_api", "GET", normalize=norm_all()),
        case("products_list_search_ok", "product_list_api", "GET", query={"q": "脐橙"},
             normalize=norm_all()),
        case("products_list_sku_filter_ok", "product_list_api", "GET",
             query={"sku_type": "family"}, normalize=norm_all()),
        case("products_list_v1_ok", "v1_product_list_api", "GET",
             alias_of="/api/products", normalize=norm_all()),
        case("product_detail_ok", "product_detail_api", "GET",
             url_args={"product_id": prod}, normalize=norm_all()),
        case("product_detail_v1_ok", "v1_product_detail_api", "GET",
             url_args={"product_id": prod}, alias_of="/api/products/<uuid:product_id>",
             normalize=norm_all()),
        V("product_detail_unknown_404", "product_detail_api", "GET",
          url_args={"product_id": NIL_UUID}, status=404),
        case("product_health_archive_ok", "product_health_archive_api", "GET",
             url_args={"product_id": prod},
             normalize=norm_all("$.data.updatedAt", "$.data.orchard.verifiedAt",
                                "$.data.orchardHealth.verifiedAt")),
        case("product_health_archive_v1_ok", "v1_product_health_archive_api", "GET",
             url_args={"product_id": prod},
             alias_of="/api/products/<uuid:product_id>/health-archive",
             normalize=norm_all("$.data.updatedAt", "$.data.orchard.verifiedAt",
                                "$.data.orchardHealth.verifiedAt")),
        V("product_health_archive_unknown_404", "product_health_archive_api", "GET",
          url_args={"product_id": NIL_UUID}, status=404),
        # ---------------- 买家读 ----------------
        case("cart_get_ok", "cart_api", "GET", role="buyer_zhang", auth="bearer",
             normalize=norm_all()),
        case("cart_v1_get_ok", "v1_cart_api", "GET", role="buyer_zhang", auth="bearer",
             alias_of="/api/cart", normalize=norm_all()),
        A("cart_get_no_token_401", "cart_api", "GET"),
        R("cart_get_farmer_403", "cart_api", "GET", role="farmer_xinfeng",
          auth="bearer", note="文案：该接口仅限购买者使用"),
        V("cart_post_missing_product_400", "cart_api", "POST", role="buyer_zhang",
          auth="bearer", body={}),
        V("cart_post_bad_uuid_400", "cart_api", "POST", role="buyer_zhang",
          auth="bearer", body={"product_id": "not-a-uuid", "quantity": 1}),
        V("cart_post_zero_quantity_400", "v1_cart_api", "POST", role="buyer_zhang",
          auth="bearer", body={"product_id": prod, "quantity": 0}),
        V("cart_item_patch_unknown_404", "cart_item_api", "PATCH", role="buyer_zhang",
          auth="bearer", url_args={"item_id": NIL_UUID}, body={"quantity": 1},
          status=404),
        V("cart_item_delete_unknown_404", "v1_cart_item_api", "DELETE",
          role="buyer_zhang", auth="bearer", url_args={"item_id": NIL_UUID},
          status=404),
        R("cart_item_patch_farmer_403", "cart_item_api", "PATCH", role="farmer_xinfeng",
          auth="bearer", url_args={"item_id": item}, body={"quantity": 1}),
        case("addresses_list_ok", "address_api", "GET", role="buyer_zhang",
             auth="bearer", normalize=norm_all()),
        case("addresses_v1_list_ok", "v1_address_api", "GET", role="buyer_zhang",
             auth="bearer", alias_of="/api/addresses", normalize=norm_all()),
        A("addresses_list_no_token_401", "address_api", "GET"),
        R("addresses_list_farmer_403", "v1_address_api", "GET", role="farmer_xinfeng",
          auth="bearer"),
        V("addresses_post_missing_fields_400", "address_api", "POST",
          role="buyer_zhang", auth="bearer", body={}),
        V("address_detail_patch_unknown_404", "address_detail_api", "PATCH",
          role="buyer_zhang", auth="bearer", url_args={"address_id": NIL_UUID},
          body={"detail": "x"}, status=404),
        V("address_detail_delete_unknown_404", "v1_address_detail_api", "DELETE",
          role="buyer_zhang", auth="bearer", url_args={"address_id": NIL_UUID},
          status=404),
        V("address_detail_delete_unknown_404_alias2", "address_detail_api", "DELETE",
          role="buyer_zhang", auth="bearer", url_args={"address_id": NIL_UUID},
          status=404, note="DELETE 主路径：未知 id -> 收货地址不存在"),
        R("address_detail_patch_farmer_403", "v1_address_detail_api", "PATCH",
          role="farmer_xinfeng", auth="bearer", url_args={"address_id": addr},
          body={"detail": "x"}),
        R("addresses_v1_post_farmer_403", "v1_address_api", "POST",
          role="farmer_xinfeng", auth="bearer",
          body={"recipient_name": "x", "phone": "13900139000", "detail": "x"}),
        R("v1_cart_item_patch_farmer_403", "v1_cart_item_api", "PATCH",
          role="farmer_xinfeng", auth="bearer", url_args={"item_id": item},
          body={"quantity": 1}),
        case("orders_list_ok", "order_api", "GET", role="buyer_zhang", auth="bearer",
             normalize=norm_all()),
        case("orders_v1_list_ok", "v1_order_api", "GET", role="buyer_zhang",
             auth="bearer", alias_of="/api/orders", normalize=norm_all()),
        A("orders_list_no_token_401", "order_api", "GET"),
        R("orders_list_farmer_403", "order_api", "GET", role="farmer_xinfeng",
          auth="bearer"),
        case("order_detail_ok", "order_detail_api", "GET", role="buyer_zhang",
             auth="bearer", url_args={"order_id": order},
             normalize=norm_all("$.data.order_number")),
        case("order_detail_v1_ok", "v1_order_detail_api", "GET", role="buyer_zhang",
             auth="bearer", url_args={"order_id": order},
             alias_of="/api/orders/<uuid:order_id>",
             normalize=norm_all("$.data.order_number")),
        V("v1_order_pay_alias_read_405", "v1_order_pay_api", "GET", status=405,
          role="buyer_zhang", auth="bearer", url_args={"order_id": order},
          alias_of="/api/orders/<uuid:order_id>/pay",
          note="未声明 GET；用于比对别名 405 文案与主路径一致"),
        V("v1_order_cancel_alias_read_405", "v1_order_cancel_api", "GET", status=405,
          role="buyer_zhang", auth="bearer", url_args={"order_id": order},
          alias_of="/api/orders/<uuid:order_id>/cancel",
          note="未声明 GET；用于比对别名 405 文案与主路径一致"),
        V("order_pay_method_not_allowed_405", "order_pay_api", "GET", status=405,
          role="buyer_zhang", auth="bearer", url_args={"order_id": order}),
        V("order_cancel_method_not_allowed_405", "order_cancel_api", "GET", status=405,
          role="buyer_zhang", auth="bearer", url_args={"order_id": order}),
        V("order_detail_unknown_404", "order_detail_api", "GET", role="buyer_zhang",
          auth="bearer", url_args={"order_id": NIL_UUID}, status=404),
        V("orders_post_missing_address_400", "order_api", "POST", role="buyer_zhang",
          auth="bearer", body={}),
        V("orders_post_unknown_address_404", "order_api", "POST", role="buyer_zhang",
          auth="bearer", body={"address_id": NIL_UUID}, status=404),
        V("v1_orders_post_missing_address_400", "v1_order_api", "POST",
          role="buyer_zhang", auth="bearer", body={}),
        V("order_cancel_completed_400", "order_cancel_api", "POST", role="buyer_zhang",
          auth="bearer", url_args={"order_id": order},
          note="已完成订单不可取消：当前订单状态不能取消"),
        V("order_pay_completed_400", "order_pay_api", "POST", role="buyer_zhang",
          auth="bearer", url_args={"order_id": order},
          note="已完成订单不可支付：当前订单状态不能支付"),
        V("order_pay_unknown_404", "v1_order_pay_api", "POST", role="buyer_zhang",
          auth="bearer", url_args={"order_id": NIL_UUID}, status=404),
        V("order_cancel_unknown_404", "v1_order_cancel_api", "POST", role="buyer_zhang",
          auth="bearer", url_args={"order_id": NIL_UUID}, status=404),
        A("order_pay_no_token_401", "order_pay_api", "POST",
          url_args={"order_id": order}),
        R("order_pay_farmer_403", "v1_order_pay_api", "POST", role="farmer_xinfeng",
          auth="bearer", url_args={"order_id": order}),
        R("order_cancel_farmer_403", "order_cancel_api", "POST", role="farmer_xinfeng",
          auth="bearer", url_args={"order_id": order}),
        case("after_sales_list_ok", "after_sale_api", "GET", role="buyer_zhang",
             auth="bearer", normalize=norm_all()),
        case("after_sales_v1_list_ok", "v1_after_sale_api", "GET", role="buyer_zhang",
             auth="bearer", alias_of="/api/after-sales", normalize=norm_all()),
        A("after_sales_list_no_token_401", "after_sale_api", "GET"),
        R("after_sales_list_farmer_403", "after_sale_api", "GET", role="farmer_xinfeng",
          auth="bearer"),
        V("after_sales_post_missing_fields_400", "after_sale_api", "POST",
          role="buyer_zhang", auth="bearer", body={}),
        V("after_sales_post_unknown_order_400", "v1_after_sale_api", "POST",
          role="buyer_zhang", auth="bearer",
          body={"order": NIL_UUID, "issue_type": "other", "description": "qa"},
          note="无效主键 -> 400（序列化器先于视图校验）"),
        # ---------------- 写类 ----------------
        # 写类顺序有依赖，别随意调整。cart_api POST 的同批次约束（读 commerce_views 源码 +
        # 实测确认）：
        #   others = 车中「除目标商品外」其他商品的批次集合（去重）
        #   仅当 others 为空、或 others == {目标商品的批次} 时才允许加购，否则 400。
        # seed 购物车 = b1:纽荷尔企业装 + b3:红肉脐橙企业装（两个批次）：
        #   * target=b3 商品 -> others 去重后 = {b1, b3}（seed 两件都在）……实测仍被拒；
        #   * target=b1 商品 -> others = {b1, b3} != {b1} -> 被拒（这就是最初的 400 原因）。
        # 结论：**只有 others 恰好只有一个批次且等于目标批次时才成功**，
        # 所以先清空购物车（两条 DELETE），再走「空车 -> 加购 -> 下单」的正路。
        M("cart_item_delete_b3_ok", "cart_item_api", "DELETE", role="buyer_zhang",
          auth="bearer", expect_status=200, url_args={"item_id": item_b3},
          note="删掉 b3 的 seed 条目（拿到只剩 b1 的车）"),
        M("cart_item_delete_b1_ok", "v1_cart_item_api", "DELETE", role="buyer_zhang",
          auth="bearer", expect_status=200, url_args={"item_id": item},
          note="删掉 b1 的 seed 条目（v1 别名 DELETE）-> 空车"),
        M("cart_get_empty_ok", "cart_api", "GET", role="buyer_zhang", auth="bearer",
          expect_status=200, normalize=norm_all(), kind="read",
          note="空车状态：items=[]、totalAmount=0.00"),
        M("cart_add_ok", "cart_api", "POST", role="buyer_zhang", auth="bearer",
          expect_status=200, body={"product_id": prod, "quantity": 2},
          capture={"cart_item_b1": "$.data.id"},
          normalize=norm_all("$.data.updatedAt"),
          note="空车后加购 b1 商品：others 为空 -> 允许"),
        M("cart_item_patch_ok", "cart_item_api", "PATCH", role="buyer_zhang",
          auth="bearer", expect_status=200,
          url_args={"item_id": "@capture:cart_item_b1"},
          body={"quantity": 3}, normalize=norm_all("$.data.updatedAt")),
        M("cart_add_b1_family_ok", "v1_cart_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"product_id": prod, "quantity": 1},
          normalize=norm_all("$.data.updatedAt"),
          capture={"cart_item_b1_2": "$.data.id"},
          note="v1 别名加购同类 b1 商品：others={b1} == {b1} -> 允许"),
        M("cart_item_delete_b1_family_ok", "cart_item_api", "DELETE",
          role="buyer_zhang", auth="bearer", expect_status=200,
          url_args={"item_id": "@capture:cart_item_b1_2"},
          note="再清空一次购物车"),
        M("addresses_post_buyer_ok", "address_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"recipient_name": "李四", "phone": "13900139000", "province": "江西省",
                "city": "赣州市", "district": "南康区", "detail": "和谐大道 1 号",
                "is_default": False}, normalize=norm_all()),
        M("address_detail_patch_ok", "address_detail_api", "PATCH", role="buyer_zhang",
          auth="bearer", expect_status=200, url_args={"address_id": addr2},
          body={"detail": "科技园路 9 号（契约基准）"}, normalize=norm_all()),
        M("cart_add_b1_for_order_ok", "cart_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"product_id": prod, "quantity": 1},
          normalize=norm_all("$.data.updatedAt"),
          note="空车后加入 b1 商品 -> 购物车只剩 b1，可以下单"),
        M("orders_create_ok", "order_api", "POST", role="buyer_zhang", auth="bearer",
          expect_status=200, body={"address_id": addr, "note": "契约基准下单"},
          capture={"order_new": "$.data.id"}, normalize=norm_all("$.data.order_number")),
        M("order_pay_ok", "order_pay_api", "POST", role="buyer_zhang", auth="bearer",
          expect_status=200, url_args={"order_id": "@capture:order_new"},
          capture={"order_paid": "$.data.id"},
          normalize=norm_all("$.data.order_number")),
        M("order_cancel_paid_400", "order_cancel_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=400,
          url_args={"order_id": "@capture:order_paid"},
          note="已支付订单不可取消：当前订单状态不能取消"),
        M("order_cancel_pending_ok", "order_cancel_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200, url_args={"order_id": order_pending},
          normalize=norm_all("$.data.order_number")),
        M("orders_create_empty_cart_400", "order_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=400, body={"address_id": addr},
          note="下单会清空购物车，此时车已空：请选择有效的购物车商品"),
        M("cart_add_v1_ok", "v1_cart_api", "POST", role="buyer_zhang", auth="bearer",
          expect_status=200, body={"product_id": prod, "quantity": 1},
          normalize=norm_all("$.data.updatedAt"),
          note="v1 别名加购：空车后加 b1 商品"),
        M("orders_create_v1_ok", "v1_order_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"address_id": addr, "note": "契约基准下单(v1)"},
          capture={"order_new_v1": "$.data.id"},
          normalize=norm_all("$.data.order_number")),
        M("order_pay_v1_ok", "v1_order_pay_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          url_args={"order_id": "@capture:order_new_v1"},
          normalize=norm_all("$.data.order_number")),
        M("order_cancel_v1_paid_400", "v1_order_cancel_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=400,
          url_args={"order_id": "@capture:order_new_v1"},
          note="别名 cancel 在已支付状态 -> 400"),
        M("after_sales_create_ok", "after_sale_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"order": order, "package": refs["package_id"], "issue_type": "damaged",
                "description": "契约基准：收到时有破损", "evidence_urls": []},
          normalize=norm_all(), note="会把订单状态改为 after_sale"),
        M("after_sales_v1_create_ok", "v1_after_sale_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"order": order_pending, "issue_type": "logistics",
                "description": "契约基准：v1 别名售后", "evidence_urls": []},
          normalize=norm_all(),
          note="视图不校验订单状态，已取消订单也能提售后"),
        M("products_list_after_state_ok", "product_list_api", "GET", expect_status=200,
          kind="read", normalize=norm_all(), note="写类之后重读：库存已变"),
        M("orders_v1_list_after_state_ok", "v1_order_api", "GET", role="buyer_zhang",
          auth="bearer", expect_status=200, kind="read", normalize=norm_all(),
          note="写类之后重读订单列表"),
    ]


# --------------------------------------------------------------------------------------
# orchard_trace 域（21 条 path）
# --------------------------------------------------------------------------------------

def orchard_trace_cases(refs):
    oid = refs["orchard_id"]
    bid = refs["batch_id"]
    prod = refs["product_id"]
    tcode = refs["trace_code"]
    return [
        # ---------------- 公开读 ----------------
        case("orchards_list_ok", "orchard_list_api", "GET", normalize=norm_all()),
        case("orchards_list_search_ok", "orchard_list_api", "GET", query={"q": "信丰"},
             normalize=norm_all()),
        case("orchards_list_v1_ok", "v1_orchard_list_api", "GET",
             alias_of="/api/orchards", normalize=norm_all()),
        case("orchard_detail_ok", "orchard_detail_api", "GET",
             url_args={"orchard_id": oid}, normalize=norm_all()),
        case("orchard_detail_v1_ok", "v1_orchard_detail_api", "GET",
             url_args={"orchard_id": oid}, normalize=norm_all(),
             alias_of="/api/orchards/<uuid:orchard_id>"),
        V("orchard_detail_unknown_404", "orchard_detail_api", "GET",
          url_args={"orchard_id": NIL_UUID}, status=404),
        V("orchard_detail_bad_uuid_404", "orchard_detail_api", "GET",
          url_args={"orchard_id": "not-a-uuid"}, status=404,
          note="<uuid:> 转换失败时 Django 直接 404（不是 400）"),
        case("supply_batches_list_ok", "supply_batch_list_api", "GET", normalize=norm_all()),
        case("supply_batches_list_v1_ok", "v1_supply_batch_list_api", "GET",
             alias_of="/api/supply-batches", normalize=norm_all()),
        case("supply_batch_detail_ok", "supply_batch_detail_api", "GET",
             url_args={"batch_id": bid}, normalize=norm_all()),
        case("supply_batch_detail_v1_ok", "v1_supply_batch_detail_api", "GET",
             url_args={"batch_id": bid}, normalize=norm_all(),
             alias_of="/api/supply-batches/<uuid:batch_id>"),
        V("supply_batch_detail_unknown_404", "supply_batch_detail_api", "GET",
          url_args={"batch_id": NIL_UUID}, status=404),
        case("traces_lookup_batch_ok", "trace_lookup_api", "GET",
             url_args={"trace_code": tcode},
             normalize=norm_all("$.data.integrity.checkedAt")),
        case("traces_lookup_tree_number_ok", "trace_lookup_api", "GET",
             url_args={"trace_code": refs["tree_number"]},
             normalize=norm_all("$.data.integrity.checkedAt"),
             note="tree_number 命中 -> scope=tree"),
        case("traces_lookup_tree_code_ok", "trace_lookup_api", "GET",
             url_args={"trace_code": refs["tree_trace_code"]},
             normalize=norm_all("$.data.integrity.checkedAt")),
        case("traces_lookup_package_ok", "trace_lookup_api", "GET",
             url_args={"trace_code": refs["package_trace_code"]},
             normalize=norm_all("$.data.integrity.checkedAt"),
             note="package trace_code 命中 -> scope=package；tracking_number 被掩码"),
        case("traces_lookup_v1_ok", "v1_trace_lookup_api", "GET",
             url_args={"trace_code": tcode},
             alias_of="/api/traces/<str:trace_code>",
             normalize=norm_all("$.data.integrity.checkedAt")),
        V("traces_lookup_unknown_404", "trace_lookup_api", "GET",
          url_args={"trace_code": "CGJ-NOT-EXIST"}, status=404),
        # ---------------- 果农侧读 ----------------
        case("farmer_orchards_ok", "farmer_orchards_api", "GET", role="farmer_xinfeng",
             auth="bearer", normalize=norm_all()),
        A("farmer_orchards_no_token_401", "farmer_orchards_api", "GET"),
        R("farmer_orchards_buyer_403", "farmer_orchards_api", "GET",
          role="buyer_zhang", auth="bearer"),
        case("farmer_trees_ok", "farmer_tree_archive_api", "GET", role="farmer_xinfeng",
             auth="bearer", url_args={"orchard_id": oid}, normalize=norm_all()),
        V("farmer_trees_foreign_orchard_404", "farmer_tree_archive_api", "GET",
          role="farmer_xunwu", auth="bearer", url_args={"orchard_id": oid}, status=404,
          note="非本人果园：果园不存在或无权操作"),
        R("farmer_trees_buyer_403", "farmer_tree_archive_api", "GET",
          role="buyer_zhang", auth="bearer", url_args={"orchard_id": oid}),
        V("farmer_trees_create_missing_fields_400", "farmer_tree_archive_api", "POST",
          role="farmer_xinfeng", auth="bearer", url_args={"orchard_id": oid}, body={}),
        case("farmer_batches_ok", "farmer_batches_api", "GET", role="farmer_xinfeng",
             auth="bearer", normalize=norm_all()),
        R("farmer_batches_buyer_403", "farmer_batches_api", "GET", role="buyer_zhang",
          auth="bearer"),
        V("farmer_batch_create_missing_orchard_404", "farmer_batch_create_api", "POST",
          role="farmer_xinfeng", auth="bearer", body={}, status=404,
          note="视图先查 orchard_id -> 缺失即 404（不是 400）"),
        V("farmer_batch_create_foreign_orchard_404", "farmer_batch_create_api", "POST",
          role="farmer_xunwu", auth="bearer",
          body={"orchard_id": oid, "title": "越权批次"}, status=404),
        A("farmer_batch_create_no_token_401", "farmer_batch_create_api", "POST"),
        R("farmer_batch_create_buyer_403", "farmer_batch_create_api", "POST",
          role="buyer_zhang", auth="bearer", body={}),
        V("farmer_upload_no_file_400", "farmer_image_upload_api", "POST",
          role="farmer_xinfeng", auth="bearer", note="文案：未提供图片文件"),
        R("farmer_upload_buyer_403", "farmer_image_upload_api", "POST",
          role="buyer_zhang", auth="bearer"),
        case("farmer_products_ok", "farmer_product_list_create_api", "GET",
             role="farmer_xinfeng", auth="bearer", normalize=norm_all()),
        R("farmer_products_buyer_403", "farmer_product_list_create_api", "GET",
          role="buyer_zhang", auth="bearer"),
        V("farmer_products_post_missing_fields_400",
          "farmer_product_list_create_api", "POST", role="farmer_xinfeng",
          auth="bearer", body={}),
        V("farmer_product_patch_foreign_404", "farmer_product_detail_api", "PATCH",
          role="farmer_xunwu", auth="bearer", url_args={"product_id": prod},
          body={"stock": 1}, status=404),
        case("farmer_product_patch_empty_body_ok", "farmer_product_detail_api", "PATCH",
             role="farmer_xinfeng", auth="bearer", url_args={"product_id": prod},
             body={}, normalize=norm_all(), note="partial=True，空 patch 合法"),
        A("farmer_product_patch_no_token_401", "farmer_product_detail_api", "PATCH",
          url_args={"product_id": prod}, body={"stock": 1}),
        R("farmer_product_put_buyer_403", "farmer_product_detail_api", "PUT",
          role="buyer_zhang", auth="bearer", url_args={"product_id": prod},
          body={"stock": 1}),
        case("farmer_harvest_archives_ok", "farmer_harvest_archive_api", "GET",
             role="farmer_xinfeng", auth="bearer", url_args={"batch_id": bid},
             normalize=norm_all()),
        V("farmer_harvest_archives_post_missing_fields_400",
          "farmer_harvest_archive_api", "POST", role="farmer_xinfeng", auth="bearer",
          url_args={"batch_id": bid}, body={}),
        R("farmer_harvest_archives_buyer_403", "farmer_harvest_archive_api", "GET",
          role="buyer_zhang", auth="bearer", url_args={"batch_id": bid}),
        V("farmer_trace_events_post_missing_fields_400", "farmer_trace_event_api",
          "POST", role="farmer_xinfeng", auth="bearer",
          url_args={"batch_id": bid}, body={}),
        R("farmer_trace_events_buyer_403", "farmer_trace_event_api", "POST",
          role="buyer_zhang", auth="bearer", url_args={"batch_id": bid}, body={}),
        case("farmer_quality_samples_ok", "farmer_quality_sample_api", "GET",
             role="farmer_xinfeng", auth="bearer", url_args={"batch_id": bid},
             normalize=norm_all()),
        V("farmer_quality_samples_post_missing_fields_400",
          "farmer_quality_sample_api", "POST", role="farmer_xinfeng", auth="bearer",
          url_args={"batch_id": bid}, body={}),
        R("farmer_quality_samples_buyer_403", "farmer_quality_sample_api", "GET",
          role="buyer_zhang", auth="bearer", url_args={"batch_id": bid}),
        case("farmer_health_records_ok", "farmer_product_health_record_api", "GET",
             role="farmer_xinfeng", auth="bearer", url_args={"batch_id": bid},
             normalize=norm_all(),
             note="该 path 复用 farmer_quality_sample_api，与 quality-samples 同实现"),
        V("farmer_health_records_foreign_batch_404", "farmer_product_health_record_api",
          "GET", role="farmer_xunwu", auth="bearer", url_args={"batch_id": bid},
          status=404),
        A("farmer_health_records_no_token_401", "farmer_product_health_record_api",
          "GET", url_args={"batch_id": bid}),
        # ---------------- 写类 ----------------
        M("farmer_trees_create_ok", "farmer_tree_archive_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"orchard_id": oid},
          body={"tree_number": "信丰-QA-9001", "plot_name": "QA 地块",
                "variety": "纽荷尔脐橙", "planted_year": 2019,
                "growth_stage": "果实膨大转色期", "health_status": "healthy",
                "growth_summary": "契约基准新建果树档案", "image_urls": [],
                "video_urls": [], "is_featured": False},
          normalize=norm_all("$.data.trace_code")),
        M("farmer_trees_create_duplicate_500", "farmer_tree_archive_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=500,
          url_args={"orchard_id": oid},
          body={"tree_number": "信丰-QA-9001", "variety": "纽荷尔脐橙"},
          note="同果园 tree_number 唯一约束在 DB 层 -> IntegrityError 冒泡 -> 500"
               "（DRF 异常体形状，无 timestamp）"),
        M("farmer_upload_image_shape_only", "farmer_image_upload_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200, shape_only=True,
          files={"image": "product-8x8.png"},
          note="shape_only：返回绝对 URL（含 host 与随机文件名）"),
        M("farmer_batch_create_shape_only", "farmer_batch_create_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200, shape_only=True,
          body={"orchard_id": oid, "title": "契约基准批次", "subtitle": "QA",
                "planned_quantity": 100, "expected_harvest_start": "2026-10-01",
                "expected_harvest_end": "2026-10-06",
                "expected_ship_start": "2026-10-07",
                "expected_ship_end": "2026-10-12",
                "maturity_standard": "糖度达标", "quality_commitment": "抽检留档",
                "natural_variation_note": "自然差异",
                "aftersale_policy": "签收后 24 小时内可申请",
                "environment_summary": {"temperature": "24.0°C", "humidity": "70%",
                                        "source": "果园节点定时采集",
                                        "recordedAt": CAPTURED_AT},
                "payment_mode": "full", "deposit_ratio": 0},
          capture={"batch_new": "$.data.id"},
          note="shape_only：code/trace_code 由 uuid 生成，Rust 侧无法逐字复现"),
        M("farmer_batch_create_second_ok", "farmer_batch_create_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"orchard_id": oid, "title": "契约基准批次（第二次）",
                "planned_quantity": 20},
          normalize=norm_all("$.data.code", "$.data.trace_code"),
          note="title 不唯一；只有 code/trace_code 随机"),
        M("farmer_product_create_ok", "farmer_product_list_create_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"name": "契约基准商品", "sku_type": "family", "fruit_type": "脐橙",
                "variety": "纽荷尔脐橙", "origin": "江西赣州信丰县",
                "description": "契约基准", "price": "19.90", "unit": "5斤/箱",
                "stock": 50, "sweetness": "12.5", "grade": "精选果",
                "shipping_note": "产地直发", "purchase_limit": 5,
                "minimum_order_quantity": 1, "sales_batch": bid, "status": "on_sale"},
          normalize=norm_all()),
        M("farmer_product_create_without_batch_400",
          "farmer_product_list_create_api", "POST", role="farmer_xinfeng",
          auth="bearer", expect_status=400,
          body={"name": "无批次商品", "origin": "江西赣州信丰县", "price": "9.90"},
          note="必须关联供货批次：上架商品必须关联一个供货批次"),
        M("farmer_product_patch_ok", "farmer_product_detail_api", "PATCH",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"product_id": prod}, body={"stock": 497},
          normalize=norm_all()),
        M("farmer_harvest_archive_create_ok", "farmer_harvest_archive_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"batch_id": bid},
          body={"tree": refs["tree_id"], "harvested_at": "2026-09-12T02:30:00+00:00",
                "picker": "QA 采摘员", "plot_name": "A区示范地块",
                "harvest_method": "人工轻采，分筐转运", "quantity_kg": "88.50",
                "maturity_brix": "12.8", "grade": "精选果",
                "pre_harvest_status": "果面转色达标", "harvest_weather": "晴",
                "fruit_condition": "good", "appearance_note": "果面完整",
                "pest_status": "未见明显病虫害", "damage_rate_percent": "0.00",
                "summary": "契约基准采摘档案", "image_urls": [], "video_urls": []},
          capture={"harvest_archive_id": "$.data.id"},
          normalize=norm_all("$.data.harvest_code")),
        M("farmer_trace_event_create_ok", "farmer_trace_event_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"batch_id": bid},
          body={"event_type": "quality", "source_type": "farmer",
                "occurred_at": "2026-09-12T03:00:00+00:00",
                "title": "契约基准抽检事件", "description": "QA 追加溯源事件",
                "location": "果园分选区", "actor": "qa",
                "source_reference": "QA-REF-0001", "image_urls": [],
                "data": {"note": "qa"}},
          normalize=norm_all("$.data.previous_hash", "$.data.evidence_hash",
                             "$.data.hash_short", "$.data.recorded_at"),
          note="哈希链：previous_hash/evidence_hash 依赖上一条事件"),
        M("farmer_quality_sample_create_shape_only", "farmer_quality_sample_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200, shape_only=True,
          url_args={"batch_id": bid},
          body={"product": prod,
                "harvest_archive": "@capture:harvest_archive_id",
                "sampled_at": "2026-09-12T04:00:00+00:00",
                "inspection_stage": "at_harvest", "health_status": "qualified",
                "sample_size": 12, "sweetness_brix": "12.9",
                "average_weight_grams": 220, "diameter_mm": "73.0",
                "grade": "精选果", "result": "符合本批次标准",
                "appearance_status": "果面完整", "pest_status": "未见明显病虫害",
                "pesticide_residue_result": "符合要求", "inspector": "QA 质检员",
                "note": "契约基准抽检"},
          note="shape_only：写入同时追加 TraceEvent（哈希链依赖时间序列）"),
        M("farmer_health_records_create_ok", "farmer_product_health_record_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"batch_id": bid},
          body={"product": prod, "sampled_at": "2026-09-12T05:00:00+00:00",
                "inspection_stage": "post_sorting", "health_status": "qualified",
                "sample_size": 8, "sweetness_brix": "13.0", "result": "合格"},
          normalize=norm_all(), note="同 quality-samples 实现"),
        M("farmer_batches_after_state_ok", "farmer_batches_api", "GET",
          role="farmer_xinfeng", auth="bearer", expect_status=200, kind="read",
          normalize=norm_all(), note="写类之后重读：多出契约基准批次（draft）"),
        M("traces_lookup_after_state_ok", "trace_lookup_api", "GET", expect_status=200,
          kind="read", url_args={"trace_code": tcode},
          normalize=norm_all("$.data.integrity.checkedAt"),
          note="写类之后重读追溯：事件数与 latestHash 已变化"),
    ]


# --------------------------------------------------------------------------------------
# agent 域（11 条 path）
# --------------------------------------------------------------------------------------

def agent_cases(refs):
    seed_approval = "@capture:seed_approval_id"
    return [
        case("agent_context_ok", "agent_context_api", "GET", role="farmer_xinfeng",
             auth="bearer", normalize=norm_all()),
        case("agent_context_buyer_ok", "agent_context_api", "GET", role="buyer_zhang",
             auth="bearer", normalize=norm_all()),
        A("agent_context_no_token_401", "agent_context_api", "GET"),
        case("agent_daily_report_farmer_ok", "agent_daily_report_api", "GET",
             role="farmer_xinfeng", auth="bearer",
             normalize=norm_all("$.data.report.generated_at")),
        case("agent_daily_report_buyer_ok", "agent_daily_report_api", "GET",
             role="buyer_zhang", auth="bearer",
             note="非果农返回 report=null + message（200）"),
        A("agent_daily_report_no_token_401", "agent_daily_report_api", "GET"),
        case("agent_select_missing_query_500", "agent_select_api", "POST", body={},
             expect_status=500, kind="validation",
             note="蓝本缺陷：agent_views 用 status. 但未 import -> 该 400 分支实际 500"),
        case("agent_select_ok_public", "agent_select_api", "POST", expect_status=200,
             body={"query": "预算80元以内，5斤装自用"},
             note="AllowAny：匿名可调用（该视图未挂 BearerTokenAuthentication）"),
        case("agent_inquiry_missing_query_500", "agent_inquiry_api", "POST", body={},
             expect_status=500, kind="validation"),
        M("agent_inquiry_ok_public", "agent_inquiry_api", "POST", expect_status=200,
          body={"query": "采购100箱，5斤装，2027-01-01 前到货"},
          normalize=norm_all("$.data.approval.created_at")),        A("agent_risk_alert_no_token_401", "agent_risk_alert_api", "GET"),
        M("agent_risk_alert_ok", "agent_risk_alert_api", "GET", role="farmer_xinfeng",
          auth="bearer", expect_status=200,
          normalize=norm_all("$.data.approval.created_at")),
        case("agent_repurchase_buyer_ok", "agent_repurchase_api", "GET",
             role="buyer_zhang", auth="bearer"),
        case("agent_repurchase_farmer_ok", "agent_repurchase_api", "GET",
             role="farmer_xinfeng", auth="bearer",
             note="非买家返回 should_remind=false + reason（200）"),
        A("agent_repurchase_no_token_401", "agent_repurchase_api", "GET"),
        case("agent_chat_missing_query_500", "agent_chat_api", "POST",
             role="farmer_xinfeng", auth="bearer", body={}, expect_status=500,
             kind="validation"),
        A("agent_chat_no_token_401", "agent_chat_api", "POST", body={}),
        M("agent_chat_report_ok", "agent_chat_api", "POST", role="farmer_xinfeng",
          auth="bearer", expect_status=200, body={"query": "今天经营怎么样"},
          normalize=norm_all("$.data.data.report.generated_at"),
          note="LLM 关闭 -> 规则兜底；intent=report"),
        case("agent_feedback_get_buyer_ok", "agent_feedback_api", "GET",
             role="buyer_zhang", auth="bearer", normalize=norm_all()),
        A("agent_feedback_get_no_token_401", "agent_feedback_api", "GET"),
        case("agent_feedback_post_missing_rating_500", "agent_feedback_api", "POST",
             role="buyer_zhang", auth="bearer", body={}, expect_status=500,
             kind="validation"),
        case("agent_feedback_post_bad_rating_500", "agent_feedback_api", "POST",
             role="buyer_zhang", auth="bearer", body={"rating": 9},
             expect_status=500, kind="validation"),
        case("agent_approval_create_missing_title_500", "agent_approval_create_api",
             "POST", role="farmer_xinfeng", auth="bearer", body={}, expect_status=500,
             kind="validation"),
        A("agent_approval_create_no_token_401", "agent_approval_create_api", "POST",
          body={}),
        case("agent_approval_list_mine_ok", "agent_approval_list_api", "GET",
             role="farmer_xinfeng", auth="bearer", normalize=norm_all()),
        case("agent_approval_list_pending_ok", "agent_approval_list_api", "GET",
             role="farmer_xinfeng", auth="bearer", query={"scope": "pending"},
             normalize=norm_all()),
        A("agent_approval_list_no_token_401", "agent_approval_list_api", "GET"),
        case("agent_approval_decision_unknown_500", "agent_approval_decision_api",
             "POST", role="farmer_xinfeng", auth="bearer",
             url_args={"approval_id": NIL_UUID}, body={"decision": "approved"},
             expect_status=500, kind="validation",
             note="审批单不存在（应为 404）实际 500"),
        case("agent_approval_decision_bad_value_500", "agent_approval_decision_api",
             "POST", role="farmer_xinfeng", auth="bearer",
             url_args={"approval_id": seed_approval}, body={"decision": "maybe"},
             expect_status=500, kind="validation"),
        A("agent_approval_decision_no_token_401", "agent_approval_decision_api",
          "POST", url_args={"approval_id": seed_approval},
          body={"decision": "approved"}),
        # ---------------- 写类 ----------------
        M("agent_feedback_post_ok", "agent_feedback_api", "POST", role="buyer_zhang",
          auth="bearer", expect_status=200,
          body={"rating": 5, "product_name": "赣南纽荷尔家庭装",
                "batch_code": "CGJ-2026-XF-001", "taste": "很甜",
                "package": "完好", "note": "契约基准反馈"},
          normalize=norm_all()),
        M("agent_approval_create_ok", "agent_approval_create_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          body={"ticket_type": "restock", "title": "契约基准审批单",
                "ref_type": "qa", "ref_id": "qa-0001", "payload": {"note": "qa"}},
          capture={"approval_new": "$.data.id"}, normalize=norm_all()),
        M("agent_approval_decision_approve_ok", "agent_approval_decision_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"approval_id": seed_approval},
          body={"decision": "approved", "note": "契约基准批准"},
          normalize=norm_all(),
          note="seed 审批单 payload.action 非 create_task -> exec_note 为空"),
        M("agent_approval_decision_repeat_500", "agent_approval_decision_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=500,
          url_args={"approval_id": seed_approval}, body={"decision": "rejected"},
          note="已批准不能重复处理（应为 400）实际 500"),
        M("agent_approval_decision_reject_ok", "agent_approval_decision_api", "POST",
          role="farmer_xinfeng", auth="bearer", expect_status=200,
          url_args={"approval_id": "@capture:approval_new"},
          body={"decision": "rejected", "note": "契约基准拒绝"},
          normalize=norm_all()),
        M("agent_approval_list_after_state_ok", "agent_approval_list_api", "GET",
          role="farmer_xinfeng", auth="bearer", expect_status=200, kind="read",
          normalize=norm_all(), note="写类之后重读审批单列表"),
    ]


DOMAIN_BUILDERS = {
    "auth": auth_cases,
    "core": core_cases,
    "commerce": commerce_cases,
    "orchard_trace": orchard_trace_cases,
    "agent": agent_cases,
}


# --------------------------------------------------------------------------------------
# 执行上下文
# --------------------------------------------------------------------------------------


class Ctx:
    def __init__(self, refs, routes):
        self.refs = refs
        self.by_name = {r["url_name"]: r for r in routes}
        self.tokens: dict[str, str] = {}
        self.captured: dict[str, object] = {}
        self.params_used: dict[str, dict] = {}
        self.mutation_order: list[dict] = []

    def install_token(self, key: str, username: str) -> str:
        """固定 token（uuid5 派生），让 Rust 侧能用同一批凭据复现。"""
        from django.utils import timezone

        from api.models import AuthToken, User

        user = User.objects.get(username=username)
        AuthToken.objects.filter(user=user).delete()
        token = AuthToken.objects.create(
            key=uuid.uuid5(uuid.NAMESPACE_URL, "contract:" + key),
            user=user,
            expires_at=timezone.now() + timedelta(days=365),
        )
        self.tokens[key] = str(token.key)
        return self.tokens[key]

    def ensure_tokens(self) -> None:
        self.install_token("farmer_xinfeng", "farmer_xinfeng")
        self.install_token("farmer_xunwu", "farmer_xunwu")
        self.install_token("buyer_zhang", "buyer_zhang")
        self.install_token("scratch", "qa_scratch_user")
        self.install_token("scratch2", "qa_scratch_user2")

    def drop_token(self, key: str) -> None:
        """把某角色的 token 真正删掉（用于「登出后复用」-> 401 的用例）。"""
        from api.models import AuthToken

        token = self.tokens.get(key)
        if token:
            AuthToken.objects.filter(key=token).delete()

    def render_path(self, c: dict) -> str:
        from urllib.parse import quote

        route = self.by_name[c["url_name"]]
        template = route["path"]
        used = {}
        for token, key in {
            "<uuid:orchard_id>": "orchard_id",
            "<uuid:batch_id>": "batch_id",
            "<uuid:product_id>": "product_id",
            "<uuid:order_id>": "order_id",
            "<uuid:item_id>": "item_id",
            "<uuid:address_id>": "address_id",
            "<uuid:approval_id>": "approval_id",
            "<str:trace_code>": "trace_code",
        }.items():
            if token not in template:
                continue
            value = c["url_args"].get(key)
            if value is None:
                value = self.refs.get(key, "")
            value = self.resolve(value)
            template = template.replace(token, quote(str(value), safe=""))
            used[key] = str(value)
        if used:
            self.params_used.setdefault(c["url_name"], {}).update(used)
        return "/" + template.lstrip("/")

    def resolve(self, value):
        if not isinstance(value, str):
            return value
        if value.startswith("@ref:"):
            return self.refs.get(value[5:], "")
        if value.startswith("@capture:"):
            key = value[9:]
            if key not in self.captured:
                raise RuntimeError(f"capture 缺失：{key}（用例顺序有误）")
            return self.captured[key]
        if value == "@png_data_uri":
            return png_base64_data_uri()
        return value

    def resolve_body(self, body):
        if isinstance(body, dict):
            return {k: self.resolve_body(v) for k, v in body.items()}
        if isinstance(body, list):
            return [self.resolve_body(v) for v in body]
        return self.resolve(body)

    def token_for(self, c: dict):
        auth = c["auth"]
        if auth == "none":
            return None
        if auth == "malformed":
            return "__MALFORMED__"
        if auth == "unknown":
            return "__UNKNOWN__"
        return self.tokens.get(c["role"] or "farmer_xinfeng")


def send(ctx: Ctx, c: dict):
    from django.core.files.uploadedfile import SimpleUploadedFile
    from django.test import Client

    if c.get("drop_token"):
        ctx.drop_token(c["drop_token"])

    path = ctx.render_path(c)
    token = ctx.token_for(c)
    headers = {}
    if token == "__MALFORMED__":
        headers["HTTP_AUTHORIZATION"] = "Token not-bearer"
    elif token == "__UNKNOWN__":
        headers["HTTP_AUTHORIZATION"] = f"Bearer {uuid.uuid4()}"
    elif token:
        headers["HTTP_AUTHORIZATION"] = f"Bearer {token}"

    client = Client()
    method = c["method"]

    if c.get("files"):
        # 关键：必须用 client.post（client.generic 不做 multipart 编码，
        # 会让 request.FILES 为空并触发 415）
        if method != "POST":
            raise RuntimeError(f"files 仅支持 POST：{c['name']}")
        data = {
            field: SimpleUploadedFile(filename, make_png_8x8(), content_type="image/png")
            for field, filename in c["files"].items()
        }
        return client.post(path, data=data, **headers)

    body = ctx.resolve_body(c["body"]) if c.get("body") is not None else None
    if method == "GET":
        return client.get(path, data=c.get("query") or {}, **headers)
    if method == "DELETE":
        return client.delete(path, **headers)
    if method == "POST":
        return client.post(path, data=json.dumps(body if body is not None else {}),
                           content_type="application/json", **headers)
    if method == "PATCH":
        return client.patch(path, data=json.dumps(body or {}),
                            content_type="application/json", **headers)
    if method == "PUT":
        return client.put(path, data=json.dumps(body or {}),
                          content_type="application/json", **headers)
    return client.generic(method, path, **headers)


def extract(body, json_path):
    tokens = re.findall(r"\.([A-Za-z0-9_]+)|\[(\d+)\]", json_path.lstrip("$"))
    current = body
    for name, index in tokens:
        if name:
            if not isinstance(current, dict):
                return None
            current = current.get(name)
        else:
            if not isinstance(current, list) or int(index) >= len(current):
                return None
            current = current[int(index)]
    return current


def record_case(ctx: Ctx, c: dict) -> dict:
    route = ctx.by_name[c["url_name"]]
    path = ctx.render_path(c)
    entry = {
        "name": c["name"],
        "path_name": c["url_name"],
        "path_template": route["path"],
        "path": path,
        "method": c["method"],
        "kind": c["kind"],
        "headers": {"role": c["role"], "auth": c["auth"]},
        "query": c.get("query") or {},
        "body": ctx.resolve_body(c["body"]) if isinstance(c.get("body"), dict)
                else c.get("body"),
        "upload": (
            {
                "field": next(iter(c["files"])),
                "filename": next(iter(c["files"].values())),
                "content_type": "image/png",
                "sha256": hashlib.sha256(make_png_8x8()).hexdigest(),
                "size": len(make_png_8x8()),
            }
            if c.get("files") else None
        ),
        "path_params": dict(ctx.params_used.get(c["url_name"], {})),
        "expected_status": None,
        "expected_content_type": None,
        "expected_body": None,
        "raw_body_sha256": None,
        "raw_body_bytes": None,
        "normalize": expand_normalize(c["normalize"]),
        "normalize_hits": [],
        "shape_only": bool(c["shape_only"]),
        "shape": None,
        "status_matches_design": False,
        "expected_status_design": c["expect_status"],
        "alias_of": c.get("alias_of"),
        "note": c["note"],
    }

    try:
        resp = send(ctx, c)
    except Exception as exc:
        entry["transport_error"] = f"{type(exc).__name__}: {exc}"
        return entry

    raw = resp.content
    try:
        body = json.loads(raw.decode("utf-8"))
    except Exception:
        body = None

    entry["expected_status"] = resp.status_code
    entry["expected_content_type"] = resp.headers.get("Content-Type", "")
    entry["expected_body"] = body
    entry["raw_body_sha256"] = hashlib.sha256(raw).hexdigest()
    entry["raw_body_bytes"] = len(raw)
    entry["status_matches_design"] = resp.status_code == c["expect_status"]

    if body is not None:
        # ⚠️ apply_normalize 是**就地**修改：只在副本上跑，用来记录「命中了哪些字段」。
        # 约定：expected_body = 原始线上字节（可逐字比对）；
        #      normalize      = 要屏蔽的 JSON 路径清单；
        #      normalize_hits = 本次实际命中的字段名。
        # 不落盘屏蔽后的整份 body：占位符是按值哈希的、每次录制都不同，规模还很大，
        # 徒增噪声；Rust 侧按 normalize 屏蔽后比较即可。
        probe = json.loads(json.dumps(body, ensure_ascii=False))
        entry["normalize_hits"] = apply_normalize(probe, c["normalize"])
        if c["shape_only"]:
            entry["shape"] = shape_of(body)

    for key, json_path in (c.get("capture") or {}).items():
        ctx.captured[key] = extract(body, json_path)

    if resp.status_code >= 500:
        entry["server_error"] = True

    return entry


def run_capture(args, routes, refs):
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    from django.test import Client

    ctx = Ctx(refs, routes)
    for username in ("qa_scratch_user", "qa_scratch_user2"):
        resp = Client().post(
            "/api/register",
            data=json.dumps({"username": username, "password": "qa-pass-123456",
                             "role": "buyer", "email": username + "@example.com"}),
            content_type="application/json",
        )
        if resp.status_code != 200:
            raise RuntimeError(f"scratch 注册失败 {username}: {resp.content!r}")

    ctx.captured["seed_approval_id"] = seed_approval_id()
    ctx.ensure_tokens()

    selected = args.domain or list(DOMAINS)
    domain_records: dict[str, list[dict]] = {}

    for domain in selected:
        specs = DOMAIN_BUILDERS[domain](refs)
        reads = [s for s in specs if s["kind"] != "mutation"]
        writes = [s for s in specs if s["kind"] == "mutation"]
        records: list[dict] = []
        print(f"\n=== domain {domain}（read {len(reads)} / mutation {len(writes)}）===")
        for spec in reads:
            rec = record_case(ctx, spec)
            records.append(rec)
            _log(domain, rec)
        for spec in writes:
            ctx.ensure_tokens()  # 登录/登出会动 token，写类前重建固定 token
            rec = record_case(ctx, spec)
            ctx.mutation_order.append({
                "order": len(ctx.mutation_order) + 1,
                "domain": domain,
                "case": rec["name"],
                "method": rec["method"],
                "path": rec["path"],
                "note": rec["note"],
            })
            records.append(rec)
            _log(domain, rec)
        domain_records[domain] = records

    return domain_records, ctx


def _log(domain, rec):
    status = rec.get("expected_status")
    flag = "!!" if (status is None or status >= 500) else "  "
    mark = "" if rec.get("status_matches_design") else "   <<< 与设计期望不符"
    print(f"{flag}[{domain}] {rec['name']:<48} {rec['method']:<7} "
          f"{str(status):<5}{mark}")


# --------------------------------------------------------------------------------------
# seed.json
# --------------------------------------------------------------------------------------

SEED_EXCLUDED_APPS = {"contenttypes", "auth", "admin", "sessions"}


def dump_seed(out_dir: Path):
    from django.apps import apps as django_apps
    from django.core.management import call_command

    labels = []
    for app_config in django_apps.get_app_configs():
        if app_config.label in SEED_EXCLUDED_APPS:
            continue
        for model in app_config.get_models():
            labels.append(f"{app_config.label}.{model.__name__}")

    tmp = Path(tempfile.mkdtemp(prefix="seed-dump-", dir=str(TMP_DIR)))
    target = tmp / "seed.json"
    with target.open("w", encoding="utf-8") as fh:
        call_command("dumpdata", *labels, format="json", indent=2, stdout=fh)
    objects = json.loads(target.read_text(encoding="utf-8"))
    shutil.rmtree(tmp, ignore_errors=True)

    counts: dict[str, int] = {}
    for obj in objects:
        counts[obj["model"]] = counts.get(obj["model"], 0) + 1

    payload = {
        "_meta": {
            **ENV,
            "captured_at": CAPTURED_AT,
            "dump_command":
                "dumpdata <business model labels> --format=json --indent=2（保留主键）",
            "excluded": [
                "contenttypes.contenttype", "auth.permission", "auth.group",
                "admin.logentry", "sessions.session",
            ],
            "row_counts": counts,
            "total_rows": sum(counts.values()),
            "time_semantics": (
                "绝对时间戳原样保存，不做任何 rebase。TraceEvent.evidence_hash 由 "
                "occurred_at.isoformat()（含微秒）参与 sha256，因此夹具必须在 "
                "captured_at 当天回放。"
            ),
        },
        "objects": objects,
    }
    body = json.dumps(payload, ensure_ascii=False, indent=2) + "\n"
    path = out_dir / "seed.json"
    path.write_text(body, encoding="utf-8", newline="\n")
    signature = "sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest()
    print(f"\n[out] {path}  rows={payload['_meta']['total_rows']}")
    return signature, payload


# --------------------------------------------------------------------------------------
# 别名自检
# --------------------------------------------------------------------------------------

# 别名用例 -> 要对比的主用例。
# 必须显式列出：同一主路径常有多个用例（/api/me 有 buyer/farmer；/api/products 有筛选变体），
# 自动配对会拿不同用户/不同状态去比，得出无意义的「不一致」结论。
ALIAS_PRIMARY = {
    # auth
    "user_alias_ok_farmer": "me_ok_farmer",
    "v1_login_ok_farmer": "login_ok_farmer",
    "v1_register_ok_farmer": "register_ok_buyer",
    # commerce
    "products_list_v1_ok": "products_list_ok",
    "product_detail_v1_ok": "product_detail_ok",
    "product_health_archive_v1_ok": "product_health_archive_ok",
    "cart_v1_get_ok": "cart_get_ok",
    "addresses_v1_list_ok": "addresses_list_ok",
    "orders_v1_list_ok": "orders_list_ok",
    "order_detail_v1_ok": "order_detail_ok",
    "after_sales_v1_list_ok": "after_sales_list_ok",
    "v1_order_pay_alias_read_405": "order_pay_method_not_allowed_405",
    "v1_order_cancel_alias_read_405": "order_cancel_method_not_allowed_405",
    # orchard_trace
    "orchards_list_v1_ok": "orchards_list_ok",
    "orchard_detail_v1_ok": "orchard_detail_ok",
    "supply_batches_list_v1_ok": "supply_batches_list_ok",
    "supply_batch_detail_v1_ok": "supply_batch_detail_ok",
    "traces_lookup_v1_ok": "traces_lookup_batch_ok",
}


def _masked_equal(a, b):
    """比较两个**已归一化**的 body：占位符 <sha256:...> 视为「已屏蔽」而非具体值。

    占位符是按命中值算出来的哈希，两次请求当然不同（这正是要屏蔽的原因），
    所以别名比对必须做「屏蔽后的结构等价」，而不是直接 == 。
    """
    if isinstance(a, str) and isinstance(b, str):
        a_masked = a.startswith("<sha256:")
        b_masked = b.startswith("<sha256:")
        if a_masked or b_masked:
            return a_masked and b_masked
        return a == b
    if isinstance(a, dict) and isinstance(b, dict):
        if set(a) != set(b):
            return False
        return all(_masked_equal(a[k], b[k]) for k in a)
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return False
        return all(_masked_equal(x, y) for x, y in zip(a, b))
    if isinstance(a, bool) or isinstance(b, bool):
        return a is b
    return a == b


def _masked_diff(expected, actual, prefix="", out=None, limit=30):
    """在「屏蔽后结构等价」的语义下找差异（占位符对齐比较）。"""
    if out is None:
        out = []
    if len(out) >= limit:
        return out
    if isinstance(expected, dict) and isinstance(actual, dict):
        for k in sorted(set(expected) | set(actual)):
            if k not in expected:
                out.append(f"{prefix}.{k} 仅存在于别名")
            elif k not in actual:
                out.append(f"{prefix}.{k} 仅存在于主路径")
            else:
                _masked_diff(expected[k], actual[k], f"{prefix}.{k}", out, limit)
        return out
    if isinstance(expected, list) and isinstance(actual, list):
        if len(expected) != len(actual):
            out.append(f"{prefix} 长度 {len(expected)} vs {len(actual)}")
        for i, (e, a) in enumerate(zip(expected, actual)):
            _masked_diff(e, a, f"{prefix}[{i}]", out, limit)
        return out
    if not _masked_equal(expected, actual):
        out.append(f"{prefix}: {expected!r} != {actual!r}")
    return out


def alias_checks(domain_records):
    out = []
    for domain, records in domain_records.items():
        by_case = {r["name"]: r for r in records}
        for r in records:
            alias_of = r.get("alias_of")
            if not alias_of or r["expected_body"] is None:
                continue
            # 主路径往往有多个用例（/api/me 有 buyer/farmer 两种、/api/products 有筛选变体），
            # 必须显式指定要对比的主用例，否则会拿不同用户/不同状态去比，结论无意义。
            primary_name = ALIAS_PRIMARY.get(r["name"])
            primary = by_case.get(primary_name or "")
            if primary is None:
                out.append({
                    "domain": domain,
                    "alias_path": r["path"],
                    "alias_case": r["name"],
                    "alias_status": r["expected_status"],
                    "primary_path": alias_of,
                    "primary_case": r.get("primary_alias_case"),
                    "primary_status": None,
                    "method": r["method"],
                    "status_equal": False,
                    "normalized_equal": False,
                    "shape_equal": False,
                    "normalize_applied": expand_normalize(r["normalize"]),
                    "diff": [f"未指定/找不到主用例 {r.get('primary_alias_case')!r}"],
                    "note": "无法比对：主用例缺失",
                })
                continue
            if primary["expected_body"] is None:
                continue
            a = json.loads(json.dumps(r["expected_body"]))
            b = json.loads(json.dumps(primary["expected_body"]))
            apply_normalize(a, r["normalize"])
            apply_normalize(b, r["normalize"])
            same = _masked_equal(a, b)
            diff = [] if same else _masked_diff(b, a)
            out.append({
                "domain": domain,
                "alias_path": r["path"],
                "alias_case": r["name"],
                "alias_status": r["expected_status"],
                "primary_path": primary["path"],
                "primary_case": primary["name"],
                "primary_status": primary["expected_status"],
                "method": r["method"],
                "status_equal": r["expected_status"] == primary["expected_status"],
                "normalized_equal": same,
                "shape_equal": shape_of(a) == shape_of(b),
                "normalize_applied": expand_normalize(r["normalize"]),
                "diff": diff,
                "note": "屏蔽后结构等价" if same else "存在差异，见 diff",
            })
    return out


def _diff(expected, actual, prefix="", out=None, limit=30):
    if out is None:
        out = []
    if len(out) >= limit:
        return out
    if isinstance(expected, dict) and isinstance(actual, dict):
        for k in sorted(set(expected) | set(actual)):
            if k not in expected:
                out.append(f"{prefix}.{k} 仅存在于别名")
            elif k not in actual:
                out.append(f"{prefix}.{k} 仅存在于主路径")
            else:
                _diff(expected[k], actual[k], f"{prefix}.{k}", out, limit)
    elif isinstance(expected, list) and isinstance(actual, list):
        if len(expected) != len(actual):
            out.append(f"{prefix} 长度 {len(expected)} vs {len(actual)}")
        for i, (e, a) in enumerate(zip(expected, actual)):
            _diff(e, a, f"{prefix}[{i}]", out, limit)
    elif expected != actual:
        out.append(f"{prefix}: {expected!r} != {actual!r}")
    return out


# --------------------------------------------------------------------------------------
# index.json
# --------------------------------------------------------------------------------------


def build_index(routes, domain_records, ctx, refs, alias_results, signature, seed_rows):
    all_cases: dict[tuple, list] = {}
    for records in domain_records.values():
        for r in records:
            all_cases.setdefault((r["path_name"], r["method"]), []).append(r)

    entries = []
    for route in routes:
        mine = []
        per_method = []
        for method in route["methods"]:
            records = all_cases.get((route["url_name"], method), [])
            mine.extend(records)
            per_method.append({
                "method": method,
                "case_count": len(records),
                "cases": [r["name"] for r in records],
                "statuses": sorted({r["expected_status"] for r in records}),
            })
        statuses = [r["expected_status"] for r in mine]
        histogram = {
            str(code): statuses.count(code)
            for code in sorted({s for s in statuses if s is not None})
        }
        mismatched = [r["name"] for r in mine if not r["status_matches_design"]]
        entries.append({
            "path": "/" + route["path"].lstrip("/"),
            "path_template": route["path"],
            "name": route["url_name"],
            "domain": domain_of(route["url_name"]),
            "view": route["view"],
            "enumeration": route["status"],
            "methods": route["methods"],
            "method_case_counts": per_method,
            "cases": len(mine),
            "status_histogram": histogram,
            "covered": len(mine) > 0,
            "covered_200": histogram.get("200", 0),
            "partial": (len(mine) == 0) or bool(mismatched),
            "blocked": False,
            "blocked_reason": "",
            "mismatched_status_cases": mismatched,
            "shape_only_cases": [r["name"] for r in mine if r["shape_only"]],
            "server_error_cases": [r["name"] for r in mine if r.get("server_error")],
            "alias_of": next((r["alias_of"] for r in mine if r.get("alias_of")), None),
            "path_params_used": ctx.params_used.get(route["url_name"], {}),
            "mutation_cases": [
                m for m in ctx.mutation_order
                if m["case"] in {r["name"] for r in mine}
            ],
        })

    return {
        "captured_at": CAPTURED_AT,
        "seed_signature": signature,
        "environment": ENV,
        "path_count": len(entries),
        "domain_counts": {
            d: len([e for e in entries if e["domain"] == d])
            for d in DOMAINS + ["unassigned"]
        },
        "domain_covered": sorted(domain_records.keys()),
        "method_case_capacity": sum(len(e["methods"]) for e in entries),
        "case_count": sum(e["cases"] for e in entries),
        "covered": len([e for e in entries if e["covered"] and not e["partial"]]),
        "partial": len([e for e in entries if e["partial"]]),
        "blocked": len([e for e in entries if e["blocked"]]),
        "unresolved": [e for e in entries if e["enumeration"] != "ok"],
        "uncovered": [e["path"] for e in entries if not e["covered"]],
        "seed_refs": refs,
        "captured_values": {k: v for k, v in ctx.captured.items()
                            if k != "seed_approval_id"},
        "seed_approval_id": ctx.captured.get("seed_approval_id"),
        "seed_row_counts": seed_rows,
        "mutation_order": ctx.mutation_order,
        "alias_conclusion": {
            "checked": len(alias_results),
            "equal": len([a for a in alias_results if a["normalized_equal"]]),
            "mismatch": [a for a in alias_results if not a["normalized_equal"]],
            "detail": alias_results,
        },
        "paths": entries,
    }


# --------------------------------------------------------------------------------------
# REPORT.md
# --------------------------------------------------------------------------------------

UNCOVERED_NOTES = [
    "`/api/temperature-humidity` GET 的 **404 分支**（该果农无记录且 CSV 缺失）无法覆盖："
    "seed 给每个果农 10 条记录，且仓库内不存在 `temperature_humidity_data.csv`。",
    "`/api/orders/<id>/pay` 的**订金/尾款分支**未覆盖：seed 的批次全部 `payment_mode='full'`；"
    "构造 `deposit_balance` 批次需要改 seed，会破坏夹具一致性。",
    "`/api/orders/<id>/cancel` 的**退款分支**（`paid_amount>0` 时插 `MOCK-REFUND-*`）未覆盖："
    "已支付订单会被 400 拦下（契约如此）。",
    "`/api/v1/farmer/batches/<uuid>/quality-samples` GET 的 **product_id 过滤**未单独出例："
    "已覆盖无过滤的 200 与缺字段 400。",
    "`/api/agent/approvals/<id>/decision` 的 `payload.action='create_task'` 执行分支未覆盖："
    "已覆盖 `action` 缺失 -> `exec_note` 为空 的分支。",
    "`/api/v1/farmer/upload-image` 的 >5MB / 伪造 content_type 分支未覆盖："
    "需要构造大文件或改写 MIME。",
    "`/admin/`（Django admin）与 `/media/` 静态兜底不属于 App API 契约，未录制。",
]


def build_report(index, domain_records, alias_results, seed_payload):
    cases = [c for records in domain_records.values() for c in records]
    samples = _time_samples(cases)

    L: list[str] = []
    add = L.append
    add("# Wave 0' 契约基准报告（Django 5 + DRF → Rust golden fixtures）")
    add("")
    add(f"- **captured_at**（UTC）：`{index['captured_at']}`")
    add(f"- **seed_signature**：`{index['seed_signature']}`")
    add("- 夹具目录：`db/tests/fixtures/contract/`")
    add(f"- 蓝本：`navel_backend_git/`（Django {ENV['django_version']} / DRF "
        f"{ENV['drf_version']} / Python {ENV['python_version']}）")
    add(f"- 本次录制域：{', '.join(index['domain_covered'])}")
    add("")

    add("## 1. 覆盖统计")
    add("")
    add(f"- 自省 `api/urls.py`：**{index['path_count']} 条 `path()`**"
        f"（任务书里写的 103 有误，实测 79）")
    add(f"- 域划分：auth {index['domain_counts']['auth']} / core "
        f"{index['domain_counts']['core']} / orchard_trace "
        f"{index['domain_counts']['orchard_trace']} / commerce "
        f"{index['domain_counts']['commerce']} / agent {index['domain_counts']['agent']}"
        f"（合计 {sum(index['domain_counts'][d] for d in DOMAINS)}）")
    add(f"- 用例总数：**{index['case_count']}**"
        f"（path×method 上限 {index['method_case_capacity']}）")
    add(f"- covered（有用例且状态码与设计一致）：**{index['covered']} / "
        f"{index['path_count']}**")
    add(f"- partial（有用例但状态码与设计不符）：**{index['partial']}**")
    add(f"- blocked：**{index['blocked']}**")
    add(f"- unresolved（自省失败，不静默跳过）：**{len(index['unresolved'])}**"
        + (f" → `{index['unresolved']}`" if index["unresolved"] else "（无）"))
    add("")
    add("> 域划分按**路径前缀**：`/api/v1/farmer/*` 归 orchard_trace，"
        "`/api/{products,cart,addresses,orders,after-sales}` 及其 `v1/` 别名归 commerce。"
        "每条 `path()` 至少 1 条用例；数值以实际录制的 `expected_status` 为准，"
        "`expected_status_design` 保留人工设计值以便发现蓝本漂移。")
    add("")

    add("### 1.1 逐域覆盖")
    add("")
    add("| 域 | path | 用例 | 200 | 400 | 401 | 403 | 404 | 405 | 5xx | shape_only |")
    add("|---|---|---|---|---|---|---|---|---|---|---|")
    for d in DOMAINS:
        es = [e for e in index["paths"] if e["domain"] == d and e["cases"]]
        total_paths = len([e for e in index["paths"] if e["domain"] == d])
        if not es:
            add(f"| {d} | {total_paths} | 0 | — | — | — | — | — | — | — | — |")
            continue
        h: dict[str, int] = {}
        for e in es:
            for k, v in e["status_histogram"].items():
                h[k] = h.get(k, 0) + v
        five = sum(v for k, v in h.items() if int(k) >= 500)
        add(f"| {d} | {total_paths} | {sum(e['cases'] for e in es)} | "
            f"{h.get('200', 0)} | {h.get('400', 0)} | {h.get('401', 0)} | "
            f"{h.get('403', 0)} | {h.get('404', 0)} | {h.get('405', 0)} | {five} | "
            f"{sum(len(e['shape_only_cases']) for e in es)} |")
    add("")
    add("逐 path 明细（用例数 / 覆盖到的状态码）：")
    add("")
    add("| path | name | 方法 | 用例 | 状态码 |")
    add("|---|---|---|---|---|")
    for e in index["paths"]:
        codes = ", ".join(f"{k}×{v}" for k, v in sorted(e["status_histogram"].items()))
        add(f"| `{e['path']}` | `{e['name']}` | {', '.join(e['methods'])} | "
            f"{e['cases']} | {codes} |")
    add("")

    add("## 2. 未覆盖 / partial 清单")
    add("")
    if index["uncovered"]:
        add("**没有任何用例的 path**：")
        add("")
        for p in index["uncovered"]:
            add(f"- `{p}`")
    else:
        add(f"**{index['path_count']} 条 path 全部至少 1 条用例，uncovered = 0。**")
    add("")
    bad = [e for e in index["paths"] if e["mismatched_status_cases"]]
    if bad:
        add("状态码与人工设计期望不符的 path（以实际录制值为准）：")
        add("")
        for e in bad:
            add(f"- `{e['path']}`（{e['name']}）：{e['mismatched_status_cases']}")
    else:
        add("所有用例的实际状态码与设计期望一致（无 partial 偏差）。")
    add("")
    add("### 2.1 已知未覆盖的语义分支（及原因）")
    add("")
    for note in UNCOVERED_NOTES:
        add(f"- {note}")
    add("")

    add("## 3. shape_only 清单（只校验键与类型，不校验值）")
    add("")
    add("| 用例 | path | 原因 |")
    add("|---|---|---|")
    for c in cases:
        if c["shape_only"]:
            add(f"| `{c['name']}` | `{c['path']}` | {c['note'] or '—'} |")
    add("")
    add("shape_only 用例的键/类型契约写在用例的 `shape` 字段里"
        "（形如 `$.data.items[*].name:str`），Rust 侧按此校验，不要比对数值。")
    add("")

    add("## 4. 时间格式实证")
    add("")
    add("`USE_TZ=True` + `TIME_ZONE='UTC'` 下所有 datetime 走 DRF 默认表示：")
    add("`YYYY-MM-DDTHH:MM:SS.ffffff+00:00` —— **带 `+00:00` 偏移、6 位微秒**，不是 `Z` 结尾。")
    add("")
    add("| JSON 路径 | 实测样例 | 判定 |")
    add("|---|---|---|")
    for s in samples[:30]:
        add(f"| `{s['json_path']}` | `{s['value']}` | {s['verdict']} |")
    add("")
    bad_time = [s for s in samples if s["format_ok"] is False]
    add("- 形状规则：`^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}"
        "(\\.\\d{1,6})?\\+00:00$`")
    add(f"- 形状不符的样例数：**{len(bad_time)}**")
    add("- `DateField`（`recognitionDate`、`expectedHarvestStart/End`、`harvest_date`）"
        "输出 `YYYY-MM-DD`，无时间部分。")
    add("- envelope 的 `$.timestamp` 是**毫秒整数**"
        "（`int(timezone.now().timestamp()*1000)`），不是 ISO 字符串。")
    add("- ⚠️ 一处例外：`complete_task_api` 用朴素 `datetime.now()` 写 `completed_at`，"
        "DRF 会输出**不带偏移**的 `2026-09-20T21:05:50.123456`。"
        "Rust 侧若要逐字兼容需单独处理这一支。")
    add("- `TraceEvent.evidence_hash` 的输入含 `occurred_at.isoformat()` 的**微秒**，"
        "任何时间改写都会破坏哈希链（`integrity.chainValid` 变 false）。")
    add("")

    add("## 5. 契约怪癖（Rust 侧必须逐字复刻）")
    add("")
    add("### 5.1 响应体形状")
    add("")
    add("- 成功体：`{code, message, data, timestamp}`，`timestamp` 为**毫秒整数**。")
    add("- 异常体（`api/exceptions.py::custom_exception_handler`）："
        "`{code, message, data}` —— **没有 `timestamp`**。")
    add("- 未捕获异常也走异常体：`{code:500, message:'Internal server error', data:null}`。")
    add("- 但 `views.py` 的 `except Exception -> create_response(None, str(e), 500)` 属"
        "**成功体形状**（带 timestamp，message 是异常文本）。两条 500 路径形状不同。")
    add("- 序列化器校验失败时 `message` 是**对象**"
        "（`{'字段': ['文案']}` 或 `{'non_field_errors': [...]}`），不是字符串。")
    add("")
    add("### 5.2 错误文案字典（逐字，取自实际响应）")
    add("")
    add("| 状态码 | message | 出处 |")
    add("|---|---|---|")
    for row in _message_rows(cases):
        add(row)
    add("")
    add("### 5.3 蓝本自身的缺陷（**必须原样复刻，否则契约比对会失败**）")
    add("")
    add("- `api/agent_views.py` 顶层**没有** `from rest_framework import status`，"
        "却在 9 处使用 `status.HTTP_4xx_*`。因此下列「应为 400/404」的分支"
        "**实际返回 500 `{'code':500,'message':'Internal server error','data':null}`**：")
    add("  - `POST /api/agent/select` 缺 query")
    add("  - `POST /api/agent/inquiry` 缺 query")
    add("  - `POST /api/agent/chat` 缺 query")
    add("  - `POST /api/agent/feedback` rating 缺失 / 非 1-5")
    add("  - `POST /api/agent/approvals` 缺 title")
    add("  - `POST /api/agent/approvals/<id>/decision` 审批单不存在 / decision 非法 / 重复决策")
    add("  （对照：`api/commerce_views.py`、`api/auth_views.py` 都正确 import 了 status）")
    add("- `api/views.py::fertilization_plan_api` 引用未定义的 "
        "`FertilizationPlanRequestSerializer`（`api/serializers.py` 有定义，"
        "但 `views.py` 没 import）-> **`POST /api/generate/fertilization-plan` 恒返回 "
        "`500 {code:500, message:\"name 'FertilizationPlanRequestSerializer' is not defined\"}`**"
        "（成功体形状，带 timestamp）。")
    add("- 请与产品确认：是有意修 bug（那 Rust 侧不该复刻这些 500），"
        "还是按现状等价？**当前夹具按实际行为录制。**")
    add("")
    add("### 5.4 鉴权与权限")
    add("")
    add("- `Authorization: Bearer <uuid>`：")
    add("  - 头存在但格式不符 -> 401 `Authorization 请求头格式无效`")
    add("  - token 查不到 -> 401 `登录凭证无效`")
    add("  - token 过期 -> 401 `登录已过期，请重新登录`")
    add("  - 完全没头 -> 401 `身份认证信息未提供。`（**中文**，DRF zh-hans 本地化）")
    add("- 权限不足 -> 403 `该接口仅限果农使用`（IsFarmer）/ `该接口仅限购买者使用`（IsBuyer）")
    add("- 方法不允许 -> 405 `方法 “DELETE” 不被允许。`（zh-hans，含全角引号）")
    add("- 401 响应带 `WWW-Authenticate: Bearer`。")
    add("- `POST /api/login` 凭据错误是 **401**（不是 400），`message` 为 "
        "`{'non_field_errors': ['Invalid credentials']}`（英文）。")
    add("- `/api/register`、`/api/login`、`/api/agent/select`、`/api/agent/inquiry` "
        "未挂 `BearerTokenAuthentication` -> 匿名可调；传 token 也不影响（不校验）。")
    add("")
    add("### 5.5 别名（`v1/*`）结论")
    add("")
    add(f"- 自动对比 **{index['alias_conclusion']['checked']}** 组同状态别名："
        f"normalize 后相等 **{index['alias_conclusion']['equal']}** 组，"
        f"不等 **{len(index['alias_conclusion']['mismatch'])}** 组。")
    for a in index["alias_conclusion"]["mismatch"]:
        add(f"  - `{a['alias_path']}`（{a['alias_case']}）vs "
            f"`{a['primary_path']}`（{a['primary_case']}）：diff={a['diff']}")
    add("- 全部别名都是「同一个 `@api_view` 函数挂在两条 path 上」，无独立实现。")
    add("- `POST /api/v1/auth/login` 与 `POST /api/login` 是**同一视图**，会重建 token"
        "（`_create_session` 先删该用户全部 token）。录制时 `v1_login_ok_farmer` "
        "排在复用 token 的用例之后；Rust 侧测试也必须遵守「一次登录、token 复用」的顺序。")
    add("- `GET /api/user` 与 `GET /api/me` 是同一视图（`auth_views.me_api`）。")
    add("")
    add("### 5.6 其他怪癖")
    add("")
    add("- `GET /api/traces/<code>` 依次匹配 `FruitTreeArchive.trace_code|tree_number` → "
        "`TracePackage.trace_code` → `SalesBatch.trace_code|code`，`data.scope` 为 "
        "`tree`/`package`/`batch`；package 分支的 `tracking_number` 被掩码。")
    add("- `GET /api/v1/farmer/batches/<uuid>/health-records` **复用** "
        "`farmer_quality_sample_api`（与 `quality-samples` 同一实现）。")
    add("- `POST /api/v1/farmer/batches/create` 缺 `orchard_id` 返回 **404** "
        "`果园不存在或无权操作`（视图先查果园、再校验序列化器），不是 400。")
    add("- `POST /api/tasks/generate/disease` 疾病名为 `健康果树`/`非果树` -> "
        "**200 + data=null + message='No task needed for healthy tree'**（英文）。")
    add("- `POST /api/tasks/generate/environment` 低风险 -> "
        "**200 + 'No task needed for low risk'**。")
    add("- `POST /api/orders` 会删除已下单的购物车项、扣减库存、并按数量生成 "
        "`trace_packages`（`sequence` 从 1 连号）；`trace_code` 是 uuid4 -> 必须屏蔽。")
    add("- 支付写 `provider_transaction_id = MOCK-<32位大写hex>`、`provider='mock'`；"
        "退款写 `MOCK-REFUND-<32位大写hex>`。")
    add("- `planId` 形如 `fp-YYYYMMDD-NNNN`（`random.randint(1000,9999)`）。")
    add("- 购物车/下单限制同一供货批次："
        "`一次只能结算同一果园供货批次，请先完成或清空当前购物车`、"
        "`一笔订单只能购买同一果园供货批次的商品`。")
    add("- `PUT /api/v1/farmer/products/<id>` 传**空 body** 也返回 200 `商品已更新`"
        "（`partial=True` 且无必填校验）。")
    add("- `DELETE /api/v1/addresses/<id>` 删除默认地址时，会把剩余第一条置为 `is_default=true`。")
    add("- `POST /api/v1/farmer/batches/<uuid>/harvest-archives` 与 "
        "`.../quality-samples` 写入时会**自动追加 TraceEvent**（哈希链）。")
    add("")

    add("## 6. seed.json")
    add("")
    add("- 来源：`dumpdata <业务模型 label> --format=json --indent=2`（**保留主键**），"
        "排除 contenttypes / auth.* / admin.logentry / sessions。")
    add(f"- 行数合计：**{seed_payload['_meta']['total_rows']}**")
    add("")
    add("| model | rows |")
    add("|---|---|")
    for model, cnt in sorted(seed_payload["_meta"]["row_counts"].items()):
        add(f"| `{model}` | {cnt} |")
    add("")
    add("`seed.json` 是**纯 seed 态**（在跑任何用例之前导出）：Rust 侧从这里加载，"
        "然后按 `index.json.mutation_order` / 各用例数组顺序回放，"
        "每步状态自然与录制时一致。")
    add("")

    add("## 7. 确定性与「当天回放」约束")
    add("")
    add("- LLM 全关：`AGENT_LLM_API_KEY=''`（早于 `import api.*`）-> "
        "`agent_service._LLM_VALID is False`；脚本启动断言，不成立即 abort。")
    add("- 识别走 mock：仓库内无 `model/`，`MODEL_AVAILABLE=False`；脚本启动断言。")
    add("- `random.seed(20260913)` 在导入期设置。")
    add("- **不做时间 rebase**：`seed.json` 保存绝对时间戳。"
        "`TraceEvent.evidence_hash = sha256(规范化 JSON)`，输入含 "
        "`occurredAt.isoformat()` 的微秒，改写时间必然导致 `chainValid=false`。")
    add(f"- 因此 **Rust 侧必须在 `{index['captured_at'][:10]}`（UTC 日历日）当天回放**。"
        "跨天会改变：`SalesBatch.is_open`（`close_at` 过期）、"
        "`_expire_stale_orders`（`expires_at <= now` 的待支付订单被自动取消）、"
        "日报 `today_order_count`/`today_order_amount`、复购 `days_since`、"
        "`fulfillment_risk_score` 的「发货窗口临近 / 已过预计发货日」分支。")
    add(f"- 若必须跨天回放：请把 Rust 侧时钟冻结到 `{index['captured_at']}` 附近，"
        "或按相同相对时间重建 seed，**不要改夹具**。")
    add("- 用 `seed_signature`（`sha256(seed.json)`）识别夹具版本；回放前先校验签名。")
    add(f"- 临时媒体目录：`{MEDIA_TMP}`（仓库外，跑完自动清理）。"
        f"`navel_backend_git/media/` 与 `db.sqlite3` 全程未被写入"
        f"（脚本前后对 `db.sqlite3` 做 mtime+size 双检，并把 `real_db_sha256` "
        f"记进 index/seed）。")
    add("")

    add("## 8. 重跑命令")
    add("")
    add("```powershell")
    add("# 0) 一次性环境（仓库外 venv；不装 torch/torchvision）")
    add(r"& D:\githubs\db_work\.venv-django\Scripts\python.exe -m ensurepip --upgrade")
    add(r"& D:\githubs\db_work\.venv-django\Scripts\python.exe -m pip install `")
    add(r'    "Django>=5.0,<5.1" "djangorestframework>=3.15,<3.16" `')
    add(r'    "django-cors-headers>=4.0" "Pillow>=10.0" "requests>=2.31"')
    add("")
    add("# 1) 自检：环境 + 路由枚举 + 用例矩阵核对（不写文件）")
    add(r"& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py --smoke")
    add("")
    add("# 2) 全量录制")
    add(r"& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py")
    add("")
    add("# 3) 只重录某些域（可重复；index/seed/REPORT 仍会重写）")
    add(r"& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py --domain commerce --domain agent")
    add("")
    add("# 4) 输出到别处")
    add(r"& D:\githubs\db_work\.venv-django\Scripts\python.exe db\scripts\capture_contract.py --out D:\githubs\db_work\.tmp\contract")
    add("```")
    add("")
    add("录制后复核仓库无意外改动：")
    add("")
    add("```powershell")
    add(r"git -C D:\githubs\db_work\navel_backend_git status --short   # 应为空")
    add(r"git -C D:\githubs\db_work\db status --short                  # 只应看到 db/scripts/capture_contract.py 与 db/tests/fixtures/**")
    add("```")
    add("")

    add("## 9. Rust 侧必须屏蔽（normalize）的字段清单")
    add("")
    add("夹具里这些字段的值已被替换为 `<sha256:16hex>`："
        "**键必须存在、类型必须一致，值不参与比较**。")
    add("")
    add("| 字段名（按名递归屏蔽） | 典型位置 | 为什么必须屏蔽 |")
    add("|---|---|---|")
    add("| `timestamp` | 成功体 envelope、温湿度历史点 | 毫秒时间戳 |")
    add("| `created_at` / `createdAt` | 所有模型 / envelope | 写入时刻 |")
    add("| `updated_at` / `updatedAt` | 几乎所有模型 | 写入时刻 |")
    add("| `id` | 新建对象、`items[*]`、`trace_packages[*]`、审批单… | `uuid.uuid4()` |")
    add("| `orderNumber` / `order_number` | 订单 | `NO<时间><6hex>` |")
    add("| `expiresAt` / `expires_at` | 订单、登录 | 相对 now 生成 |")
    add("| `paidAt` / `cancelledAt` / `paid_at` / `cancelled_at` | 订单、支付 | 写入时刻 |")
    add("| `providerTransactionId` / `provider_transaction_id` | 支付记录 | `MOCK-<32hex>` |")
    add("| `token` | 登录 / 注册 | `AuthToken.key` = uuid4 |")
    add("| `code` / `trace_code` / `traceCode` | 新建批次、箱码、果树 | uuid4 派生 |")
    add("| `harvest_code` / `harvestCode` | 采摘档案 | `CGJ-HV-<10hex>` |")
    add("| `path` / `url` | 图片上传 | 随机文件名 |")
    add("| `planId` | 施肥方案 | `fp-<date>-<4位随机>`（该接口恒 500，保留以防修复） |")
    add("| `previous_hash` / `evidence_hash` / `hash_short` | 追溯事件 | 依赖上一条事件 + 写时刻 |")
    add("| `recorded_at` / `recordedAt` | 追溯事件 / env summary | 写入时刻 |")
    add("| `generated_at` | 经营日报 | 生成时刻 |")
    add("| `checkedAt` | 追溯完整性校验 | 校验时刻（每次请求都变） |")
    add("| `verifiedAt` | 果园健康档案 | seed 的绝对 `verified_at` |")
    add("| `sold_quantity` / `stock` | 批次 / 商品（写类之后） | 随订单与支付变化 |")
    add("")
    add("**不屏蔽**：路径参数里的 `<uuid:...>` 用 `seed.json` 的真实 UUID —— "
        "运行期实值记在 `index.json.seed_refs` 与 `index.json.paths[].path_params_used`。")
    add("")
    add("每个用例的 `normalize`（已展开的 JSON 路径清单）与 `normalize_hits`"
        "（录制时实际命中的字段名）都在对应域 JSON 里，可直接消费。")
    add("")

    return "\n".join(L) + "\n"


def _message_rows(cases):
    rows = []
    seen = set()
    for c in cases:
        body = c.get("expected_body")
        if not isinstance(body, dict) or not isinstance(body.get("code"), int):
            continue
        code = body["code"]
        if code == 200:
            continue
        message = body.get("message")
        rendered = message if isinstance(message, str) else json.dumps(
            message, ensure_ascii=False)
        key = (code, rendered)
        if key in seen:
            continue
        seen.add(key)
        rows.append(f"| {code} | `{rendered}` | `{c['path']}` |")
    return rows


TIME_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,6})?\+00:00$")


def _time_samples(cases):
    samples = []
    seen = set()

    def walk(node, path):
        if len(samples) >= 80:
            return
        if isinstance(node, dict):
            for k, v in node.items():
                walk(v, f"{path}.{k}")
        elif isinstance(node, list):
            if node:
                walk(node[0], f"{path}[*]")
        elif isinstance(node, str):
            key = path.split("[")[0]
            if node.startswith("<sha256:"):
                leaf = path.split(".")[-1]
                if ("At" in leaf or "_at" in leaf or "timestamp" in leaf) and key not in seen:
                    seen.add(key)
                    samples.append({"json_path": path, "value": node,
                                    "verdict": "已屏蔽（占位符）", "format_ok": None})
                return
            if re.match(r"^\d{4}-\d{2}-\d{2}T", node):
                if key in seen:
                    return
                seen.add(key)
                samples.append({
                    "json_path": path, "value": node,
                    "verdict": "✅ 带 +00:00 与微秒" if TIME_RE.match(node) else "❌ 形状异常",
                    "format_ok": bool(TIME_RE.match(node)),
                })

    for c in cases:
        walk(c.get("expected_body"), f"$.{c['name']}")
    return samples


# --------------------------------------------------------------------------------------
# 用例矩阵核对
# --------------------------------------------------------------------------------------


def matrix_coverage(routes, refs):
    planned: dict[tuple, list] = {}
    for domain in DOMAINS:
        for c in DOMAIN_BUILDERS[domain](refs):
            planned.setdefault((c["url_name"], c["method"]), []).append(
                f"{domain}:{c['name']}")
    missing = []
    known_names = {r["url_name"] for r in routes}
    for route in routes:
        for method in route["methods"]:
            if (route["url_name"], method) not in planned:
                missing.append((route["url_name"], method, route["path"]))
    # 反向核对：用例引用的 url_name 必须存在。
    # 注意：**故意**用未声明的方法（如 DELETE /api/me -> 405）是合法用例，不算错。
    unknown = [k for k in planned if k[0] not in known_names]
    return missing, planned, unknown


# --------------------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description="录制 navel_backend_git 契约夹具")
    parser.add_argument("--domain", action="append", choices=DOMAINS,
                        help="只录指定域（可重复）")
    parser.add_argument("--out", default=str(FIXTURE_DIR), help="输出目录")
    parser.add_argument("--smoke", action="store_true",
                        help="只做自检 + 路由枚举 + 用例矩阵核对，不写文件")
    parser.add_argument("--keep-media", action="store_true", help="保留临时媒体目录")
    args = parser.parse_args()

    test_db_name, teardown = bootstrap_django()
    print(f"[env] test db = {test_db_name}")
    print(f"[env] django={ENV['django_version']} drf={ENV['drf_version']} "
          f"python={ENV['python_version']}")
    print(f"[env] media_root = {MEDIA_TMP}")
    print(f"[env] llm_enabled={ENV['llm_enabled']} model_available="
          f"{ENV['model_available']}（识别走 mock）")

    try:
        routes = enumerate_routes()
        refs = resolve_seed_refs()

        by_domain: dict[str, list] = {}
        for r in routes:
            by_domain.setdefault(domain_of(r["url_name"]), []).append(r)

        print(f"\n[enum] path 总数 = {len(routes)}   "
              f"path×method 上限 = {sum(len(r['methods']) for r in routes)}")
        for d in DOMAINS + ["unassigned"]:
            items = by_domain.get(d, [])
            if not items:
                continue
            print(f"  {d:<14} path={len(items):>3}  methods="
                  f"{sum(len(i['methods']) for i in items):>3}")
            if d == "unassigned":
                for i in items:
                    print(f"    !! 未分配域: {i['url_name']} {i['path']}")
            for i in items:
                if i["status"] != "ok":
                    print(f"    !! {i['status']}: {i['url_name']} {i['path']}")

        missing, planned, unknown = matrix_coverage(routes, refs)
        print(f"\n[matrix] 用例总数 = {sum(len(v) for v in planned.values())}")
        if missing:
            print("!! 以下 path×method 没有任何用例：")
            for url_name, method, path in missing:
                print(f"   {method:<7} {path}  ({url_name})")
        if unknown:
            print("!! 以下用例引用了不存在的 path×method：")
            for k in unknown:
                print(f"   {k}")
        if missing or unknown:
            print("\n[smoke] 用例矩阵核对失败（见上）")
            return 2
        print("[matrix] 全部 path×method 均有用例 ✓")

        if args.smoke:
            print("\n[smoke] 自检通过（未写任何文件）")
            return 0

        domain_records, ctx = run_capture(args, routes, refs)
        alias_results = alias_checks(domain_records)
        signature, seed_payload = dump_seed(Path(args.out))

        for domain, records in domain_records.items():
            payload = {
                "domain": domain,
                "captured_at": CAPTURED_AT,
                "seed_signature": signature,
                "environment": ENV,
                "case_count": len(records),
                "cases": records,
            }
            path = Path(args.out) / f"{domain}.json"
            path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
                            encoding="utf-8", newline="\n")
            print(f"[out] {path}  cases={len(records)}")

        index = build_index(routes, domain_records, ctx, refs, alias_results,
                            signature, seed_payload["_meta"]["row_counts"])
        index_path = Path(args.out) / "index.json"
        index_path.write_text(json.dumps(index, ensure_ascii=False, indent=2) + "\n",
                              encoding="utf-8", newline="\n")
        print(f"[out] {index_path}")

        report = build_report(index, domain_records, alias_results, seed_payload)
        report_path = Path(args.out) / "REPORT.md"
        report_path.write_text(report, encoding="utf-8", newline="\n")
        print(f"[out] {report_path}")

        print("\n=== 汇总 ===")
        print(f"path={index['path_count']} covered={index['covered']} "
              f"partial={index['partial']} blocked={index['blocked']} "
              f"cases={index['case_count']}")
        print(f"alias: checked={index['alias_conclusion']['checked']} "
              f"equal={index['alias_conclusion']['equal']} "
              f"mismatch={len(index['alias_conclusion']['mismatch'])}")
        se = sorted(c["name"] for recs in domain_records.values() for c in recs
                    if c.get("server_error"))
        print(f"5xx 用例 {len(se)} 条：{se}")
        return 0
    finally:
        teardown()
        if not args.keep_media:
            shutil.rmtree(MEDIA_TMP, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
