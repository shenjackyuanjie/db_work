"""D14 探针：实测蓝本 register 对 `email: ""` / 缺省 / `null` 的真实行为。

用法：
    D:\\githubs\\db_work\\.venv-django\\Scripts\\python.exe db\\scripts\\diag_register_email_blank.py
"""

import json
import os
import pathlib
import sys

os.environ["AGENT_LLM_API_KEY"] = ""
os.environ["DJANGO_SETTINGS_MODULE"] = "navel_back.settings"
sys.path.insert(0, r"D:\githubs\db_work\navel_backend_git")
sys.dont_write_bytecode = True

import django  # noqa: E402

django.setup()

from django.conf import settings  # noqa: E402

media = pathlib.Path(r"D:\githubs\db_work\.venv-django-tmp\media")
media.mkdir(parents=True, exist_ok=True)
settings.MEDIA_ROOT = media

from django.core.management import call_command  # noqa: E402
from django.db import connection  # noqa: E402
from django.test import Client  # noqa: E402
from django.test.utils import setup_test_environment  # noqa: E402

setup_test_environment()
old_config = connection.creation.create_test_db(verbosity=0)
try:
    call_command("seed_demo_data", verbosity=0)
    from api.models import User  # noqa: E402

    client = Client()
    variants = [
        ("email='' (空串)", {"email": ""}),
        ("email 缺省", {}),
        ("email=null", {"email": None}),
    ]

    for index, (label, extra) in enumerate(variants):
        username = f"w20_email_probe_{index}"
        payload = {
            "username": username,
            "password": "secret123",
            "role": "buyer",
            **extra,
        }
        response = client.post(
            "/api/register",
            data=json.dumps(payload),
            content_type="application/json",
        )
        print(f"\n=== {label} ===")
        print(f"  HTTP {response.status_code}")
        try:
            body = response.json()
            print(f"  body.code={body.get('code')}  message={body.get('message')!r}")
            data = body.get("data") or {}
            print(f"  data keys={sorted(data) if isinstance(data, dict) else type(data).__name__}")
            if isinstance(data, dict) and "user" in data:
                print(f"  data.user.email={data['user'].get('email')!r}")
            elif isinstance(data, dict):
                print(f"  data={json.dumps(data, ensure_ascii=False)[:200]}")
        except Exception as exc:  # noqa: BLE001
            print(f"  非 JSON 响应: {exc}")

        row = User.objects.filter(username=username).first()
        if row is None:
            print("  库中无该用户（注册未成功）")
        else:
            print(f"  库中 email 原始值 = {row.email!r}  (is None: {row.email is None})")

    # 直接问序列化器，看它是否放行空串
    from api.serializers import UserRegistrationSerializer  # noqa: E402

    probe = UserRegistrationSerializer(
        data={"username": "w20_ser_probe", "password": "secret123",
              "role": "buyer", "email": ""}
    )
    print(f"\n=== UserRegistrationSerializer.is_valid() = {probe.is_valid()} ===")
    print(f"  errors={probe.errors!r}")
finally:
    connection.creation.destroy_test_db(old_config)
