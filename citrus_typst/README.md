# 柑橘果园智能诊断与管理系统论文材料

## 目录结构

| 目录 | 内容 |
| --- | --- |
| `paper/` | 当前论文 Typst 源码、清洁版 PDF、图表资源和 Mermaid 配置 |
| `revision_20260818/` | 本轮大修的中文回复信、带高亮修改稿、评测材料与修改定位清单 |
| `review/` | 审稿原件和文字版审稿意见 |
| `reports/plagiarism/` | 查重报告及压缩包 |
| `reports/aigc/` | AIGC 检测报告及压缩包 |
| `archive/manuscripts/` | 修改前的 Word、PDF 和 Markdown 论文版本 |
| `archive/notes/` | 历史报告/写作材料 |
| `archive/reference/` | 历史参考文档 |

## 编译论文

```powershell
Set-Location citrus_typst\paper

# 清洁版
typst compile main.typ main.pdf

# 带黄色高亮的修改稿
typst compile --input revision-marked=true main.typ ..\revision_20260818\main_marked.pdf
```

## 更新图表

在 `paper/` 目录执行：

```powershell
bun x --package @mermaid-js/mermaid-cli mmdc -i assets\architecture.mmd -o assets\architecture.svg -c config\mermaid-config.json -p config\edge-puppeteer.json
bun x --package @mermaid-js/mermaid-cli mmdc -i assets\usecase.mmd -o assets\usecase.svg -c config\mermaid-config.json -p config\edge-puppeteer.json
```

大修材料的使用说明见 [revision_20260818/README.md](revision_20260818/README.md)。
