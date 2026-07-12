// 中文学术论文模板
// 符合标准学术论文格式要求
// 本地粗体实现，避免依赖外部包
#let fakebold(body) = text(body, weight: "bold")

// ============ 字号定义 ============
#let 一号 = 26pt
#let 小一 = 24pt
#let 小二 = 18pt
#let 三号 = 16pt
#let 四号 = 14pt
#let 小四 = 12pt
#let 五号 = 10.5pt
#let 小五 = 9pt

// ============ 字体定义 ============
#let 宋体 = ("Times New Roman", "SimSun")
#let 黑体 = ("Times New Roman", "SimHei")
#let 楷体 = ("Times New Roman", "KaiTi")

// 正文中的手工顺序编码引文标记
#let refmark(value) = super([\[#value\]])

// 论文中的纵向流程图。节点内容由 main.typ 提供，样式统一由模板管理。
#let vertical-flow(nodes, width: 82%) = {
  let parts = ()
  for (index, node) in nodes.enumerate() {
    parts.push(
      rect(
        width: width,
        inset: (x: 10pt, y: 7pt),
        radius: 3pt,
        stroke: 0.7pt + rgb("4f6b8a"),
        fill: rgb("f4f7fb"),
        align(center, node),
      )
    )
    if index < nodes.len() - 1 {
      parts.push(text(size: 13pt, fill: rgb("4f6b8a"))[↓])
    }
  }
  align(center, stack(dir: ttb, spacing: 3pt, ..parts))
}

// ============ 封面页函数 ============
#let cover-page(
  top-title: [北京信息科技大学 \ 本科生期末论文],
  main-title: [《课程名称》 \ 20XX-20XX学年第X学期期末论文],
  paper-title: "论文题目",
  college: "学院名称",
  student-name: "学生姓名",
  class-id: "班级/学号",
  teacher: "任课教师",
  score: "",
  comments: "",
) = {
  set page(
    paper: "a4",
    margin: (left: 30mm, right: 30mm, top: 25mm, bottom: 25mm),
    numbering: none,
  )

  set par(first-line-indent: 0em)

  // 顶部标题
  v(2cm)
  align(center)[
    #set text(font: 楷体, size: 小一)
    #top-title
  ]

  v(.75cm)
  // 主标题
  align(center)[
    #set text(font: 楷体, size: 一号)
    #set par(leading: 1.2em)
    #fakebold(main-title)
  ]

  v(2cm)

  // 主内容
  align(center)[
    #set text(font: 宋体, size: 四号)
    #let underline-content(content) = {
      box(
        width: 220pt,
        align(left)[
          #content
          #v(-0.9em)
          #line(length: 100%, stroke: 0.5pt)
          #v(.5em)
        ],
      )
    }

    #table(
      columns: (auto, auto),
      stroke: none,
      align: (right, left),
      row-gutter: 0.8em,
      column-gutter: 0.5em,
      [题#h(2em)目：], underline-content(paper-title),
      [学#h(2em)院：], underline-content(college),
      [学生姓名：], underline-content(student-name),
      [班级/学号：], underline-content(class-id),
      [任课教师：], underline-content(teacher),
      ..(if score != "" { ([分#h(2em)数：], underline-content(score)) } else { () }),
    )
  ]

  v(1cm)

  // 评语
  if comments != "" {
    set text(font: 楷体, size: 三号)
    align(left)[
      评#h(2em)语：#box(width: 1fr, align(left)[
        #comments
        #v(-0.15em)
        #line(length: 100%, stroke: 0.5pt)
      ])

      #v(1.5em)

      #box(
        width: 100%,
        align(left)[
          #v(-0.15em)
          #line(length: 100%, stroke: 0.5pt)
        ],
      )

    ]
  }

  pagebreak()
}

// ============ 主文档设置函数 ============
#let academic-paper(
  title: "论文标题",
  author: "作者姓名",
  school: "学校名称",
  college: "学院名称",
  abstract-content: [],
  keywords: (),
  english-title: "English Title",
  english-author: "Author Name",
  english-school: "School Name",
  english-college: "College Name",
  english-abstract-content: [],
  english-keywords: (),
  body,
) = {
  // 页面设置
  set page(
    paper: "a4",
    margin: (left: 30mm, right: 30mm, top: 25mm, bottom: 25mm),
    numbering: "1",
    number-align: center,
  )

  // 文本基本设置
  set text(
    font: 宋体,
    size: 五号,
    lang: "zh",
    region: "cn",
  )
  set par(
    first-line-indent: (amount: 2em, all: true),
    leading: 11.6pt,
    spacing: 11.6pt,
    justify: true,
  )

  // 标题样式设置
  show heading.where(level: 1): it => {
    set text(font: 黑体, size: 三号)
    set par(first-line-indent: 0em, leading: 11.6pt, spacing: 0pt)
    block(width: 100%, above: 19.1pt, below: 18.3pt)[#fakebold(it)]
  }

  show heading.where(level: 2): it => {
    set text(font: 宋体, size: 13pt)
    set par(first-line-indent: 0em, leading: 11.6pt, spacing: 0pt)
    block(width: 100%, above: 12pt, below: 18.3pt)[#fakebold(it)]
  }

  show heading.where(level: 3): it => {
    set text(font: 宋体, size: 五号, weight: "regular")
    set par(first-line-indent: 0em, leading: 11.6pt, spacing: 0pt)
    block(width: 100%, above: 12pt, below: 18.3pt)[#it]
  }

  // 一级标题编号：一、二、三、
  set heading(numbering: (..nums) => {
    let level = nums.pos().len()
    let number = nums.pos().last()
    if level == 1 {
      numbering("一、", number)
    } else if level == 2 {
      numbering("1.1", ..nums.pos())
    } else if level == 3 {
      numbering("1.1.1", ..nums.pos())
    }
  })

  // 图表设置
  // 表格单元格不继承正文首行缩进和段前距。
  show table.cell: set par(
    first-line-indent: 0em,
    leading: 12pt,
    spacing: 0pt,
  )

  show figure.where(kind: table): set figure.caption(position: top)
  show figure.where(kind: image): set figure.caption(position: bottom)

  show figure.caption: it => {
    set text(size: 小五)
    set align(center)
    it
  }

  // ============ 标题部分 ============
  align(center)[
    #set text(font: 黑体, size: 22pt)
    #set par(first-line-indent: 0em)
    #fakebold(title)
  ]

  v(1.5em)

  // ============ 作者部分 ============
  align(center)[
    #set text(font: 宋体, size: 11pt)
    #set par(first-line-indent: 0em)
    #author
  ]

  v(0.5em)

  // ============ 学校部分 ============
  align(center)[
    #set text(font: 宋体, size: 11pt)
    #set par(first-line-indent: 0em)
    （#school #college）
  ]

  v(1em)

  // ============ 摘要部分 ============
  [
    #set par(first-line-indent: 0em)
    #fakebold(text(font: 黑体, size: 三号)[中文摘要：])
  ]

  v(0.4em)

  [
    // 摘要在 Word 审阅中为 12 磅自动行距，不套用正文 11.6 磅固定行距。
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 0pt)
    #abstract-content
  ]

  v(0.5em)

  // ============ 关键词部分 ============
  [
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 14.6pt)
    #fakebold(text(font: 宋体, size: 五号)[关键词：])
    #keywords.join("；")
  ]

  v(1em)

  // ============ 英文标题、作者和单位 ============

  align(center)[
    #set text(font: "Times New Roman", size: 22pt)
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 0pt)
    #fakebold(english-title)
  ]

  v(1.5em)

  align(center)[
    #set text(font: "Times New Roman", size: 11pt)
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 0pt)
    #english-author
  ]

  v(1em)

  align(center)[
    #set text(font: "Times New Roman", size: 11pt)
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 0pt)
    (#english-school, #english-college)
  ]

  v(1em)

  // ============ 英文摘要与关键词 ============
  [
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 0pt)
    #fakebold(text(font: "Times New Roman", size: 三号)[Abstract:])
  ]

  v(18.8pt)

  [
    #set text(font: "Times New Roman", size: 五号)
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 0pt, justify: true)
    #english-abstract-content
  ]

  v(0.5em)

  [
    #set text(font: "Times New Roman", size: 五号)
    #set par(first-line-indent: 0em, leading: 12pt, spacing: 14.6pt)
    #fakebold[Keywords: ]
    #english-keywords.join("; ")
  ]

  v(1em)

  // ============ 正文部分 ============
  body
}

// ============ 参考文献计数器 ============
#let bib-counter = counter("bibliography")

// ============ 参考文献函数 ============
#let references(bib-content) = {
  v(1em)
  [
    #set par(first-line-indent: 0em, leading: 11.6pt, spacing: 11.6pt)
    #fakebold(text(font: 宋体, size: 五号)[参考文献])
  ]
  v(0.5em)
  set par(
    first-line-indent: 0em,
    hanging-indent: 2em,
    leading: 11.6pt,
    spacing: 11.6pt,
    justify: false,
  )
  set text(font: 宋体, size: 五号, cjk-latin-spacing: none)
  bib-counter.update(0)  // 重置计数器
  bib-content
}

// ============ 参考文献条目格式函数 ============
#let author-text(authors) = {
  if type(authors) == array {
    if authors.len() > 3 {
      authors.slice(0, 3).join("，") + "，等"
    } else {
      authors.join("，")
    }
  } else {
    authors
  }
}

// 带 DOI 字段的期刊文献
#let journal-ref-doi(
  authors,
  title,
  journal,
  year,
  volume: none,
  issue: none,
  pages: none,
  doi: none,
) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let author-string = author-text(authors)
    let vol-issue = if volume != none and issue != none {
      "，" + str(volume) + "(" + str(issue) + ")"
    } else if volume != none {
      "，" + str(volume)
    } else {
      ""
    }
    let page-string = if pages != none { "：" + str(pages) } else { "" }
    let doi-string = if doi != none { ". DOI: " + str(doi) } else { "" }

    [#("[" + str(num) + "]" + author-string + "." + title + "[J]." + journal + "，" + str(year) + vol-issue + page-string + doi-string + ".")]
  }
}

// 期刊
#let journal-ref(authors, title, journal, year, volume: none, issue: none, pages: none) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)

    let vol-issue = if volume != none and issue != none {
      [，#volume\(#issue\)]
    } else if volume != none {
      [，#volume]
    } else {
      []
    }

    let page-range = if pages != none {
      "：" + str(pages) + "."
    } else {
      "."
    }

    let vol-issue-text = if volume != none and issue != none {
      "，" + str(volume) + "(" + str(issue) + ")"
    } else if volume != none {
      "，" + str(volume)
    } else {
      ""
    }

    [#("[" + str(num) + "]" + authors + "." + title + "[J]." + journal + "，" + str(year) + vol-issue-text + page-range)]
  }
}

// 专著
#let book-ref(authors, title, location, publisher, year) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)

    [#("[" + str(num) + "]" + authors + "." + title + "[M]." + location + "：" + publisher + "，" + str(year) + ".")]
  }
}

// 译著
#let translated-book-ref(authors, title, translator, location, publisher, year) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)
    [#("[" + str(num) + "]" + authors + "." + title + "[M]." + translator + "." + location + "：" + publisher + "，" + str(year) + ".")]
  }
}

// 学位论文
#let thesis-ref(author, title, location, institution, year) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    [#("[" + str(num) + "]" + author + "." + title + "[D]." + location + "：" + institution + "，" + str(year) + ".")]
  }
}

// 论文集
#let proceedings-ref(author, title, editor, collection, location, publisher, year, pages: none) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let page-range = if pages != none {
      "." + str(pages) + "."
    } else {
      "."
    }

    [#("[" + str(num) + "]" + author + "." + title + "[A]." + editor + "." + collection + "[C]." + location + "：" + publisher + "，" + str(year) + page-range)]
  }
}

// 专利
#let patent-ref(applicant, title, country-patent-no, date) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    [#("[" + str(num) + "]" + applicant + "." + title + "[P]." + country-patent-no + "，" + str(date) + ".")]
  }
}

// 技术标准
#let standard-ref(code, title) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    [#("[" + str(num) + "]" + code + "." + title + "[S].")]
  }
}

// 技术报告
#let report-ref(authors, title, report-code, location, organization, year) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)
    let code = if report-code == none or report-code == "" {
      ""
    } else {
      str(report-code) + "，"
    }
    [#("[" + str(num) + "]" + authors + "." + title + "[R]." + code + location + "：" + organization + "，" + str(year) + ".")]
  }
}

// 电子文献
#let online-ref(authors, title, url, date) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)

    [#("[" + str(num) + "]" + authors + "." + title + "[EB/OL]." + url + "，" + str(date) + ".")]
  }
}

// 报纸文章
#let newspaper-ref(author, title, newspaper, date, edition: none) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let ed = if edition != none {
      "(" + str(edition) + ")"
    } else {
      ""
    }

    [#("[" + str(num) + "]" + author + "." + title + "[N]." + newspaper + "." + str(date) + ed + ".")]
  }
}

// 数据库/光盘文献
#let dbcd-ref(authors, title, location, publisher, date) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)
    [#("[" + str(num) + "]" + authors + "." + title + "[DB/CD]." + location + "：" + publisher + "，" + str(date) + ".")]
  }
}

// 其他文献
#let other-ref(authors, title, location, publisher, date) = {
  bib-counter.step()
  context {
    let num = bib-counter.get().first()
    let authors = author-text(authors)
    [#("[" + str(num) + "]" + authors + "." + title + "[Z]." + location + "：" + publisher + "，" + str(date) + ".")]
  }
}
