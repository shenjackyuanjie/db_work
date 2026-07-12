文件说明：
- main.typ：论文正文，未调用封面页；作者、学院、学号、教师等基本信息变量均为空。
- lib.typ：依据用户提供的模板库制作的本地副本，并将 cuti/fakebold 外部依赖替换为本地粗体函数。

编译：
  typst compile main.typ

架构图：
  architecture.mmd 是 Mermaid 源码，使用 Bun 调用 Mermaid CLI，并指定本机 Microsoft Edge 渲染：
  bun x --package @mermaid-js/mermaid-cli mmdc -i architecture.mmd -o architecture.svg -c mermaid-config.json -p edge-puppeteer.json

重新编译论文：
  typst compile main.typ 柑橘果园智能诊断与管理系统设计与实现_论文.pdf
