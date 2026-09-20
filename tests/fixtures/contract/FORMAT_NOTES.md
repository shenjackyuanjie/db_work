# 序列化契约实证（从 golden fixtures 统计得出，非推测）

统计方式：`python .tmp\datetime_census.py db\tests\fixtures\contract`
样本：五域 251 条用例的全部 `expected_body` 叶子值。

## 1. 时间（`DateTimeField` 等价物）

| 形态 | 出现次数 | 说明 |
|---|---|---|
| `<...>T<...>.xxxxxxZ` | **718** | 主流形态，6 位微秒 + `Z` |
| `<...>T<...>.xxxxxx+00:00` | 152 | 走 DRF `DateTimeField.to_representation` 的字段 |
| `<...>T<...>Z` | 4 | **微秒为 0 → 小数部分整体省略** |
| `<...>T<...>+00:00` | 1 | 同上，偏移形态 |
| `<...>T<...>.xxxxxx`（无偏移） | 1 类 | `complete_task_api` 写 `completed_at` 用了 `datetime.now()`（naive 本地时间），**不要**给它补偏移 |

**结论（实现必须遵守）**
1. 微秒为 0 时**不能**输出 `.000000`，要整体省略小数部分——照抄 Python `datetime.isoformat()` 的行为。
2. `Z` 与 `+00:00` 两种后缀在蓝本里**同时存在**，取决于该值是否经过 DRF 序列化器。实现按字段逐个对齐夹具实测值，`ser.rs` 同时提供两种构造器。
3. 例外：`complete_task_api` 的 `completed_at` 输出**无偏移的本地时间**（服务器 `Asia/Shanghai`），逐字复刻。

## 2. 日期（`DateField` 等价物）

`YYYY-MM-DD`，577 处。无时间部分。

## 3. 数值

| 形态 | 说明 |
|---|---|
| JSON 字符串 + 2 位小数（491 处） | `DecimalField(decimal_places=2)` → `"58.00"` |
| JSON 字符串 + 1 位小数（197 处） | `DecimalField(decimal_places=1)` → `"25.5"` |
| JSON number | `FloatField` / `IntegerField` → 原样数字 |

**结论**：`DecimalField` 一律输出**字符串**，小数位由列定义决定；`FloatField` 输出**数字**。二者不可混。

## 4. 时间戳信封

`$.timestamp` 是**毫秒整数**（如 `1789912952831`），只在**成功体**里出现；DRF 异常体 `{code,message,data:null}` **没有** `timestamp`。

## 5. `Z` / `+00:00` 的比对策略

两者语义等价、App 侧 `new Date()` 解析结果相同。L2 比对器把两种后缀**归一化后比较**，
但把差异单独计数并输出到偏差报告，避免"看起来全绿、实际格式不齐"。
