# 2026-08-18 大修材料

本目录仅存放本轮大修新增的材料；论文清洁版源文件和编译结果仍位于上一级目录。

| 路径 | 内容 |
| --- | --- |
| `point_by_point_response.md` | 中文逐条回复信 |
| `revision_locations.md` | Word 高亮位置清单 |
| `main_marked.pdf` | 黄色高亮的修改稿 |
| `../paper/assets/usecase.mmd`、`../paper/assets/usecase.svg` | 用例图源文件和生成图 |
| `evaluation/` | 独立诊断评测规范、样本清单模板和评测脚本 |

重新生成带高亮稿：

```powershell
Set-Location citrus_typst\paper
typst compile --input revision-marked=true main.typ ..\revision_20260818\main_marked.pdf
```

独立评测只可使用已确认授权、已完成标注复核且已排除训练数据重叠的样本。具体要求见 `evaluation/evaluation_protocol.md`。
