# 代码审查：`wezterm/pywezterm`（pyo3 绑定 crate）

> **这是重构前的审查记录。** 文中列出的 P0/P1 缺陷已全部修复，结构性问题已按
> `ARCHITECTURE.md` 落地（见该文档 §0 落地状态表）。保留本文是为了说明**每一条约束
> 为什么存在** —— 结论引用处（如「A2」「§5.2」）在 `ARCHITECTURE.md` 与代码注释里仍在用。
> 文中描述的文件路径与模块名是**重构前**的。

**审查范围**：`wezterm/pywezterm/` 全部源码（`Cargo.toml`、`build.rs`、`src/**`，共 5784 行）。
**方法**：全量静态阅读 + 针对可疑点做可执行复现（SVG 压缩已用 `rustc` 实测确认）+ 交叉核对 vendored 的 `wezterm-surface` / `wezterm-term` / `wezterm-cell` 实现与 `docs/PYWEZTERM_API.*.md` 的承诺。
**未做**：完整 `cargo test` / 运行时端到端（结论见文末「编译检查」）。

总体印象：这个 crate 的**并发与资源生命周期**写得比一般绑定层讲究得多——背压高低水位、`write` 放掉 GIL、close 的取消顺序、ConPTY 那几个坑，注释都交代了「为什么」而不是复述「是什么」。问题集中在**两条字节级处理的实现细节**（SVG 压缩、mode 跟踪窗口）和**每帧路径上的深拷贝**上，都是能定位、能改的小范围问题。

---

## P0 — 会产出错误结果 / 静默丢数据

### A1. SVG 压缩把非 ASCII 文本打成乱码 ★

`src/render/svg.rs:33`、`src/render/svg.rs:74`

```rust
out.push(bytes[i] as char);   // strip_empty_text
out.push(b as char);          // collapse_intertag_whitespace
```

两个函数都按**字节**遍历、再把每个字节当成一个 Latin-1 字符 `push`。ASCII 无恙，但 UTF-8 多字节字符会被拆成 N 个错字符。

实测（逐字复制这两个函数后用 `rustc` 跑）：

```
输入: <text x="0" y="0">你好，世界</text>
输出: <text x="0" y="0">Ã¤Â½Â Ã¥Â¥Â½Ã¯Â¼ÂÃ¤Â¸ÂÃ§ÂÂ</text>
```

`render_svg(compression_level)` 是公开 API，`compression_level>=1` 就调 `compress_svg`；README 的主示例正是 `t.render_svg(1)`。也就是说**任何含中文/emoji 的屏幕，用压缩渲染出来的 SVG 都是乱码**。

修复：按 `&str` 边界处理，别按字节 `push`。最小改法是命中 `<text` 时只做「跳过整段」的决策，其余一律 `out.push_str(&svg[i..next_i])`，用 `svg.char_indices()` 或 `find` 得到合法边界。

### A2. `feed` 的模式跟踪只扫每块最后 64 字节 ★

`src/term.rs:435-446`

```rust
let mut tail = self.mode_tail.lock().unwrap();
tail.extend_from_slice(data);
while tail.len() > 64 { let drop = tail.len() - 64; tail.drain(0..drop); }
update_mode_state(&self.mode, &tail);   // ← 只扫了最后 64 字节
```

`mode_tail` 的**本意**是处理跨 feed 边界的序列（注释举的例子是 `\x1b[?10` + `03h`）。但截断加在了「旧尾 + 新块」这个**拼接结果**上，于是 `data` 超过 64 字节时，前面部分**从未被扫描**。

而实际数据块远大于 64 字节：reader 线程一次给 8 KiB，README 示例 `p.read(4096)`。按 4096 算，每个块约 **98.4% 的字节被跳过**。

后果（对照 `docs/PYWEZTERM_API.zh.md` §2.5 / §2.7 的承诺）：

- `get_mouse_encoding()` 该返回 `(1000, True)` 时返回 `(0, False)`；
- `mode_restore_seq()` 少发序列，新订阅者拿不到鼠标模式/备用屏；
- `t.feed(b"\x1b[?1000h\x1b[?1006h")` 这类**单序列小块**测试能过，所以 CI 全绿——`tests/` 里确实没有任何 `mode_restore_seq` / `get_mouse_encoding` 的用例。

修复：`update_mode_state(&self.mode, data)` 全量扫；`mode_tail` 只承担「接缝」职责——扫描 `prev_tail ++ data`，然后把 `new_tail` 置为 `data` 的最后 63 字节（或 `prev_tail++data` 的最后 63 字节）。重复应用同一条 DECSET 是幂等的，多扫不会出错。

### A3. Mux reader 会整块丢弃 8 KiB 输出 ★

`src/mux.rs:518-527`

```rust
if tmp[..n].windows(10).any(|w| w == b"\x1b[?25l\x1b[8;") { continue; }
if repaint_pending_c.load(...) {
    let is_pure_repaint = tmp[..n].windows(9).any(|w| w == b"\x1b[?25l\x1b[H");
    if is_pure_repaint { continue; }
    ...
}
```

`windows(..).any(..)` 是在**整块里找**，命中就 `continue`——丢掉的是整个 8 KiB 块，不只是那 10 个字节。同一块里跟在后面的正常输出、以及要写进 `output_buf` 的录制数据，一起没了。

修复：只在标记**位于块首**且该块其余部分确实属于重绘时跳过；更稳妥的是按标记切分，只丢标记段、其余照常 feed。

---

## P1 — 会 panic / 卡死 / 状态不一致

### A4. `add_pane` 失败不回滚，之后 `render()` 越界 panic

`src/mux.rs:686-716`

```rust
{
    let mut st = self.inner.lock().unwrap();
    id = st.panes.len();
    ...改 layout、recompute_rects...
    st.focused = id;              // ← pane 还不存在就设了焦点
}
let pane = build_pane(...)?;      // ← spawn 失败在这里返回 Err
self.inner.lock().unwrap().panes.push(pane);
```

`build_pane` 失败（`spawn 失败`、`openpty 失败`）后：`layout` 已是 `Split`（`recompute_rects` 会产出 2 个 rect），但 `panes.len()` 仍是 `id`，`focused == panes.len()`。随后 `render()` 走到 `src/mux.rs:1147-1148`：

```rust
let rect = st.rects[st.focused];
let pane = &st.panes[st.focused];   // ← index out of bounds
```

`pane_rects()` 也会多报一个不存在的 pane，`pane_at()` 命中它之后所有 `pane_*(id)` 都报「pane 不存在」。

修复：`build_pane` 失败时回滚 `layout` / `focused`（或先建 pane、成功后再改状态）。

### A5. `render()` 在零 pane 时 panic

`src/mux.rs:1147`。文档已写明「`render()` 至少要有一个 pane」，但同一份 API 里其他越界路径都规规矩矩返回 `PyRuntimeError`。pyo3 会把 panic 转成 `PanicException`，对宿主是「不可恢复」的信号。建议显式返回错误，与 `get_pane` 保持一致。

### A6. `render_image` 的 `scale` 无上限 → panic

`src/render/pixmap.rs:27-32`、`src/term.rs:1088-1097`

```rust
let img_w = (cols as f64 * CELL_W as f64 * scale) as u32;
let mut pix = tiny_skia::Pixmap::new(img_w, img_h).expect("Pixmap creation failed");
```

`term.rs` 只校验了 `scale.is_finite() && scale > 0.0`。`t.render_image(1e6, "png")` 会直接 panic；`encode_image` 里还有 3 处 `.expect(...)`（`encode_png` / `from_raw` / `write_to`）同样。应当把尺寸/编码失败变成 `PyResult` 错误。

### A7. Mux reader 在**持有 terminal 锁**时做阻塞写

`src/mux.rs:534-542`

```rust
{
    let mut t = terminal_c.lock().unwrap();
    t.advance_bytes(&tmp[..n]);
    let resp = take_capture(&capture_c);
    if !resp.is_empty() {
        let _ = write_to_writer(&writer_c, &resp);   // ← 阻塞写在锁作用域内
    }
}
```

`write_to_writer` 写的是 pty；子进程不读输入时 `write_all` 会一直等下去，而此刻 terminal 锁与 writer 锁都被 reader 线程占着。宿主侧任何 `pane_text()` / `render()` 都会卡在 `terminal.lock()` 上，Python 侧还持着 GIL —— 整个进程冻住。

对比 `pty.rs::write`：那里专门用 `py.detach` 放掉 GIL 就是怕这一点，说明作者清楚「阻塞写要摘掉锁」。把 `write_to_writer` 挪出锁作用域即可（先把 `resp` 取出来，drop 掉 `t` 再写）。

### A8. 回调方向的反向死锁

`src/term.rs:1118`、`src/mux.rs:42-50`

reader 线程：持 `pane.terminal` 锁 → `advance_bytes` 触发 OSC 52 / Alert 回调 → `Python::attach` 等 GIL。
Python 主线程：持 GIL → 调 `pane_*` / `render()` → 等 `pane.terminal` 锁。

现有注释只约束了回调「不得反查终端状态」，防的是同一个方向；反方向没有防护，一开 `set_clipboard_callback` / `set_notification_callback` 就存在这个窗口。

修复：`#[pymethods]` 里取终端锁之前先 `py.detach`，让 Python 线程不再「持 GIL 等锁」。

### A9. `Surface.clear()` 清不掉内容

`src/surface_render.rs:117-123`

```rust
let (w, h) = self.surface.dimensions();
self.surface.resize(w, h);        // 同尺寸：只清 change 流
self.surface.add_change(Change::ClearScreen(ColorAttribute::Default));
```

两个事实叠加：

- `wezterm-surface/src/lib.rs:248` 的 `resize` 只在 `changes` 非空时才 `seqno+=1; changes.clear()`，**不重置 `lines`**；
- `wezterm-surface/src/line/line.rs:206` 的 `Line::resize` 用 `resize_with(width, ...)`，同宽度是 no-op，**格子内容保留**。

于是 `clear()` 之后模型里旧内容还在。下一次走 `repaint_all()` 的场合（`resize()` 改了尺寸、变更预算判定为全量、`repaint_bytes()`）会把旧内容原样画回来——「清屏」只在紧随其后的那一帧生效。

修复：把 `lines` 真正清空（`Line::resize_and_clear` 语义），或直接 `self.surface = Surface::new(w, h)`。

### A10. `clipboard_read` 存在越界读

`src/clipboard.rs:85-95`

```rust
loop {
    let u = unsafe { *ptr.add(i) };   // ← 先读
    if u == 0 { break; }
    units.push(u);
    i += 1;
    if i > 1 << 20 { break; }         // ← 后判上限
}
```

只依赖 CF_UNICODETEXT 自带 NUL 终止。剪贴板里若是别的东西（格式不对/无终止符），会一路读到 2 MiB 之外，跨过未映射页就是访问违例（进程崩）。正确做法是先用 `GlobalSize(h)` 定出真实长度再读。另外上限判断应放在读之前。

### A11. `clipboard_write` 失败路径漏掉 HGLOBAL

`src/clipboard.rs:38-53`：`SetClipboardData` 失败时 `h` 没有 `GlobalFree`（MSDN 要求调用方在失败时自行释放）；另外 `EmptyClipboard()` 在 `GlobalAlloc` 成功之前就调用了，分配失败会留下一个被清空的剪贴板。

### A12. `ConsoleInput::new` 半途失败不回滚

`src/console_input.rs:271-275`

```rust
set_mode(hout, OUTPUT_MODE)?;
set_mode(hin, INPUT_MODE)?;        // ← 这里失败，上面的改动就留下了
let orig_cp = unsafe { GetConsoleOutputCP() };
unsafe { SetConsoleOutputCP(CP_UTF8) };
```

构造函数返回 `Err` 时对象不存在，`Drop` 不会跑 —— 宿主控制台的输出模式被永久改掉且无法恢复。应在返回 `Err` 前把已改的项还原。

### A13. `close()` 与 reader 线程注册句柄的竞态

`src/pty.rs:180-187`、`src/mux.rs:487-498`：reader 线程是**进入循环之前**才把复制出来的线程句柄写进 `reader_thread`。如果 `close()` 正好在这个窗口内跑完，`cancel_reader_thread` 读到 `None` 直接返回，紧接着释放 `master` → `ClosePseudoConsole` 与 pending read 互等，正是注释里要避免的那个死锁。窗口很小（线程启动调度），但存在。可以让 `close()` 在取消失败时退化为「等 `eof` 或超时后再释放」，或让句柄在 `spawn` 侧先行复制。

### A14. 178 处 `lock().unwrap()`

任一持锁线程 panic（例如 `emit_row` 里某处越界）都会毒化锁，此后**每次**调用都 panic，宿主再也无法通过 API 恢复。对终端/状态锁建议 `lock().unwrap_or_else(|e| e.into_inner())`。

---

## P2 — 每帧路径上的性能

### B1. `lines_in_phys_range` 会**深拷贝** `Line`（含 `Vec<Cell>`）

`wezterm-term/src/screen.rs:900`：

```rust
pub fn lines_in_phys_range(&self, phys_range: Range<PhysRowIndex>) -> Vec<Line> {
    self.lines.iter().skip(..).take(..).cloned().collect()
}
```

本 crate **11 处**调用它，其中 `src/mux.rs:349`（`compose_pane`）是**每帧、每 pane** 都拷一遍——即使整帧无任何变化（`dirty` 判空只省掉了 `emit_row`，没省掉这次拷贝）。

同一个 `Screen` 上已经有借用版：

```rust
pub fn with_phys_lines<F>(&self, phys_range: Range<PhysRowIndex>, mut func: F) where F: FnMut(&[&Line]);
```

本 crate 使用次数：**0**。`logical_lines()`（`term.rs:511`）已经用了借用式的 `for_each_logical_line_in_stable_range`，说明路径是通的——只是其余读路径没跟上。

### B2. `render_changes_bytes` 每帧重建 `Capabilities` + `TerminfoRenderer`

`src/surface_render.rs:187-192`：`ProbeHints` → `Capabilities::new_with_hints` → `TerminfoRenderer::new`。这是环境探针 + terminfo 解析，Mux 每帧都跑一次。这两个值完全静态，应放 `OnceLock` 缓存（或 `thread_local`）。

### B3. 同一函数里的额外拷贝

- `src/surface_render.rs:202-215`：为了交换两个字段，`changes.iter().map(...).collect()` 复制了整个 `Change` 向量。可以就地构造渲染输入，或直接对 `&[Change]` 做一次不落地的映射。
- `src/surface_render.rs:224-227`：`out.insert(0, ..)` 连做 4 次，每次搬整个 buffer。应该先 `extend_from_slice(b"\x1b[0m")` 再续渲染结果，或预留头部。

### B4. `CellTuple` 全 String，逐格分配

`cells_of_line`（`term.rs:90-108`）每格分配 3 个 `String`（字符 + fg + bg）；`emit_row`（`mux.rs:257-298`）又每格 `clone()` 一次文本、每个空白间隙一次 `" ".repeat()`。这是每帧每 pane 每格都在堆上分配。颜色用 `ColorAttribute`（枚举，`Copy`）、文本用索引或 `SmallVec`/`ArrayString`，能把这个路径的分配数砍掉一个量级。

---

## P3 — 重复与代码质量

| # | 位置 | 问题 |
|---|---|---|
| D1 | `term.rs:71-83` vs `render/mod.rs:54-66` | `color_attr_to_string` + `rgb_hex` **逐字写了两遍** |
| D2 | `render/mod.rs:31-49` | `visible_lines` 把 `cells_of_line` 的元组构造**内联重写**了一遍，应直接调 `crate::term::cells_of_line` |
| D3 | `mux.rs:840-887` vs `term.rs:463-476, 742-765` | `pane_text`/`pane_cursor`/`pane_scroll_view` 与 `PyTerminal::text`/`cursor`/`scroll` 是同逻辑两遍；`key_down`/`key_up` 更是 **4 份**几乎逐字相同的实现（`term.rs:773-796`、`mux.rs:743-780`） |
| D4 | `surface_render.rs:22-45` vs `render/common.rs:61-87` | 两个颜色解析器语义不同：前者认 `default`/`pN`/`#rrggbb`、未知输入**静默回落 Default**；后者还认 6 位裸 hex 与 ANSI 命名色、失败返回 `None`。应统一成一个 |
| D5 | `render/svg.rs:58-72` | `if i < bytes.len() && bytes[i] == b'<' && pending_ws { }` 是**空分支死代码**；实际行为是「`>` 之后的空白一律吃掉」，`>  hello` 会变成 `>hello`（不止标签之间） |
| D6 | `mux.rs:488-498` | 嵌套两层 `#[cfg(windows)]`；且 `reader_thread_h` 在非 Windows 下是未使用变量（会告警） |
| D7 | `render/pixmap.rs:187-221` | `encode_image` 的 jpg / bmp 两个分支逐字相同，只差 `ImageFormat` |
| D8 | `render/font.rs:20, 58-63` | **`FONT_DIR_CANDIDATES = ["src/assets/fonts"]` 是相对 CWD 的路径，而仓库里根本没有这个目录**——全仓 `find` 无任何 `.ttf/.otf/.ttc`，也没有任何构建步骤去下载它。所以「首选随构建分发的 MapleMono」这条路径永不生效，`PREFERRED_FAMILIES` 是死的，CJK 一律走系统字体回退。而且相对 CWD 的写法在 wheel 场景下永远不可靠（宿主的 CWD 是别人的工程目录）——要么 `include_bytes!` 把字体编进 `.pyd`，要么像 `ensure_conpty_dir` 那样按模块目录解析 |
| D9 | `render/font.rs:184-209` | 单测断言**环境事实**：`symbol.is_some()`、`█` 宽度必须为 1、`✔` 必须为 1。没装 Segoe UI Symbol / DejaVu Sans 的机器（CI 就是）必然失败。应改成「若该字体加载成功，则断言……」 |

---

## P4 — 小问题清单

- `mux.rs:899-933` `pane_resize` 的注释说「同步布局矩形」，实现只 resize pty + terminal，**不碰 `st.rects`**。注释与代码不符。
- `mux.rs:530` `output_buf` 超过 16 MiB 后**静默丢弃**（录制会缺帧），文档未提。
- `mux.rs:1393` `close()` 关掉所有 pane 却不清空 `st.panes`，之后 `render()` 仍会合成它们；`close_pane` 用**位置**当 id，关掉 0 号后 1 号变成 0 号，宿主缓存的 id 失效。
- `mux.rs:722` `set_output_callback` 只对**之后**新建的 pane 生效（文档已写）。把 `Option<Py<PyAny>>` 换成 `Arc<Mutex<Option<..>>>` 共享给 reader 线程就能消掉这个坑，不必靠文档约束调用顺序。
- `term.rs:354` `newline` 字段注释写「自动换行模式（SM 20 / LNM）」——LNM 是换行/回车模式，不是自动换行。
- `term.rs:518` `logical_lines` 里 `stable_range.end - 1` 对空区间会下溢（目前 wezterm 不会给空区间，但没防御）。
- 光标定位格式化写了三份：`lib.rs:26` `cursor_seq`、`term.rs:592-655` `render_ansi`、`term.rs:638-652`。
- `pty.rs:236` `spawn` 调两次会覆盖 `child`（前一个 `Box<dyn Child>` 被 drop，进程不一定会被杀）。
- `pty.rs:289-323` `read` 用 2 ms `sleep` 轮询；`space` Condvar 只在「腾出空间」时通知，「有新数据」不通知 —— 每次 read 最多 2 ms 延迟且是忙等。加一个 data Condvar 更干净。
- `build.rs:28` `std::fs::copy(...).unwrap()`：IO 失败直接 panic，应给出可读的构建错误。
- `surface_render.rs:11` `use pyo3::exceptions::{PyRuntimeError};` 多余大括号。
- `Cargo.toml:24-25` `fontdb`/`fontdue` 直接写死版本，其余依赖都走 `workspace.dependencies`（不一致）。
- `mux.rs:61-63` `SplitDir` 只有一个变体，`Layout::Split` 的通用性用不上（单变体 match）。
- `tests/` 里**没有任何** `mode_restore_seq` / `get_mouse_encoding` / `render_svg` / `set_output_callback` 的覆盖 —— A1、A2 正是因此漏网。
- **测试自己的诊断信息是坏的**：`render/svg.rs:239`、`:247` 写的是 `assert!(cond, "同色 run 应合并: {svg}")`，但 crate 是 `edition = "2018"`，panic 消息**不做格式化插值**——断言失败时打印的是字面量 `{svg}` 而不是实际内容。`cargo test --no-run` 会给出 `non_fmt_panics` 告警。CI 不跑 `cargo test`（见 ARCHITECTURE.md §6），所以没人看到。顺带说明为什么 `cargo check` 是 0 告警：它不编译 `--tests`。

---

## 做得好的地方（不要改坏）

- **`pty.rs` 的背压设计**（`READ_BUFFER_HIGH/LOW` + `space` Condvar）是正确解法，而且注释解释了「这是背压传导到 PTY 的唯一途径」，还配了 `buffered_bytes()` 让不变量**可被观测**。这个思路很少见，值得保留。
- **`write` 放掉 GIL** 的理由写得很清楚，也确实必要（`write_all` 阻塞时 asyncio 会一起死）。
- **`close()` 的顺序**（置 closed → kill → 摘 writer → 取消阻塞读 → 释放 HPCON → 清缓冲 → `notify_all`）考虑周全；`read` 里「先判 closed 再判 buf」是修过竞态的痕迹。
- **`selection.rs`** 纯函数化、坐标模型（stable 行 + 列）选得对，单测里还带 CJK rewrap 回归。
- **AGENTS.md 的变更记录**：ConPTY 侧载目录、32 位 `stdcall` 栈破坏、作业对象创建时入作业（含「`UpdateProcThreadAttribute` 只登记指针不复制数据」这个坑）—— 这几条都是会反复踩的坑，记下来价值很高。

---

## 建议的修复顺序

1. **A1 SVG 字节遍历**（几行改动，直接消除乱码）——最高性价比。
2. **A2 mode 跟踪窗口**（几行改动，恢复一个已文档化功能的正确性）。
3. **A3 reader 整块丢弃**（消除静默丢数据）。
4. **A4 / A5 / A6** 三处 panic 改为返回 `PyRuntimeError`，并给 `add_pane` 加回滚。
5. **A7 / A8** 锁作用域与 GIL 方向（这是「偶发卡死」类问题，越早处理越省事）。
6. **B1 / B2** 每帧深拷贝与 terminfo 重建（Mux 场景收益最明显）。
7. **D1–D4** 消重（改完 B1/B2 再动，避免边改边重排）。
8. **D8** 字体分发策略定下来：要么把字体编进 wheel，要么删掉死掉的 `FONT_DIR_CANDIDATES` 与 `PREFERRED_FAMILIES`，别留一条永不生效的路径。
9. 补 `tests/`：模式跟踪（**大块 feed**）、`render_svg` 非 ASCII、`add_pane` 失败路径。

---

## 编译检查

`cargo check -p pywezterm`（目标目录放到仓库外，避免动 `wezterm/target`）**通过**：

```
Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 59s
```

- `pywezterm` 自身 **0 warning / 0 error**。
- 出现的告警全部来自 vendored crate：`filedescriptor`（1）、`termwiz`（1）、`portable-pty`（9）。
- 顺带一提（不在本次审查范围，但既然看到了）：`pty/src/win/psuedocon.rs` 有 3 处 `unnecessary unsafe block`，以及 `type alias HModule is never used` / `PSUEDOCONSOLE_INHERIT_CURSOR is never used` / `PSEUDOCONSOLE_RESIZE_QUIRK is never used` —— 应该是 AGENTS.md 里那次「手动 `LoadLibraryW`/`GetProcAddress` 替换 `shared_library!`」改动后留下的死代码，可以顺手清掉。

**未执行**：`cargo test`（需要真实 ConPTY 与终端环境）与 Python 端端到端。A1 已用独立 `rustc` 复现验证，其余为静态结论 + 对 vendored 实现的交叉核对。

---

# 附：架构层面的问题

前面 P0–P4 是「实现有没有错」。这一节回答「结构对不对」——这些问题的共同点是：**单点修不掉，修了还会长回来**。

先说明立场：这是一个「把 wezterm 库化成 Python 扩展」的项目，绑定层不得不承担一些上游没暴露的策略（选区、模式跟踪、渲染器）。所以下面不是「不该有这些代码」，而是**这些代码放错了层、且没有收敛成单一实现**。

## 1. 「Pane」被实现了两遍，而且已经分叉 ★

`PyPty` + `PyTerminal` 是一套宿主实现；`Mux::PaneInner` 是**另一套**——自己的 reader 线程、自己的 `master/slave/writer/child/buf/eof/closed/reader_thread`、自己的关闭五步、自己的 capture 回写。`Mux` 没有复用 `PyPty`/`PyTerminal` 的任何一行。

分叉**已经发生**，不是理论风险：

| 能力 | `PyPty` / `PyTerminal` | `Mux` 的 pane |
|---|---|---|
| 背压（高/低水位） | 有（`READ_BUFFER_HIGH/LOW` + Condvar） | **没有**，reader 直接 `advance_bytes` |
| 缓冲可观测 | `buffered_bytes()` | 无；`output_buf` 16 MiB 静默截断 |
| 模式跟踪 / `mode_restore_seq` | 有（`mode` + `mode_tail`） | **完全没有**（grep 确认 mux.rs 无相关符号） |
| SVG / 图片导出 | 有 | 无 |
| 关闭协议 | 一套 | 另一套（同样五步重写一遍） |

后果：同一个 bug 要修两遍，而且**修一套漏一套**——A2（模式跟踪）只存在于 `PyTerminal` 那一套，A3（整块丢弃）只存在于 `Mux` 那一套。上一节的 P0 里有两个是这么来的。

**建议**：抽一个 `Pane` 类型（pty + terminal + reader 线程 + 背压 + 关闭协议 + capture 回写）作为**唯一的宿主单元**；`PyPty`/`PyTerminal` 是它的薄壳，`Mux` 是「若干 `Pane` + 布局 + 焦点路由 + 合成」。这样背压、模式跟踪、导出能力对两边同时生效，`Mux` 白捡 `mode_restore_seq`。

## 2. 在绑定层手写第二个 VT 解析器 ★

`update_mode_state`（`term.rs:161-208`）是在字节流里扫 `ESC [ ... h/l` 的第二套解析器。而 `wezterm-term` 的 performer **已经解析了同一批序列**，并在 `TerminalState` 里存好了 `mouse_tracking` / `button_event_mouse` / `any_event_mouse` / alt screen / bracketed paste / focus / keyboard encoding（`term/src/terminalstate/mod.rs:317, 773-789, 2781`）。

真正的缺口只是**模型没把模式号暴露出来**：`is_mouse_grabbed()` 只返回三个布尔 OR 的结果（`terminalstate/mod.rs:773-775`），拿不到「1000 还是 1002 还是 1003」。于是绑定层选择重写解析器，代价是：

- 自带一块 64 字节的窗口缓冲 → 分块边界 bug（A2）；
- 自带一套状态，**可以与模型互相矛盾**（`is_mouse_grabbed()` 说 true、`get_mouse_encoding()` 说 0）；
- `Mux` 里干脆没有（见问题 1）；
- 约 200 行（`update_mode_state` + `apply_mode` + `TermModeState` + `mode_tail`）维护成本。

而这个项目**已经确立了「必要时改 vendored wezterm，并把变更记录写进 AGENTS.md」的先例**——`wezterm/pty` 的 `raw_cmdline`、`job_handle`、`PROC_THREAD_ATTRIBUTE_JOB_LIST` 都是这么加的，加的正是「上游没暴露、调用方需要」的东西。

**建议**：给 `TerminalState` 加一个 `pub fn mouse_reporting_mode(&self) -> Option<u16>`（以及 `sgr_mouse` / `sgr_pixels` / `cursor_visible` / `wraparound` / `insert` 的访问器），照 pty 那次的流程记一笔 AGENTS.md。这样能**删掉整个 `TermModeState` 机制**，模式状态变成模型的直接投影——单一事实来源，且不可能漂移。

原则：**绑定层不重新解析协议。** 解析只有一份，状态只有一份。

## 3. 内部 IR 是个匿名 10 元组

```rust
pub(crate) type CellTuple = (usize, String, String, String, bool, bool, bool, bool, bool, usize);
```

它被 svg、pixmap、mux 合成、term 的 text/render_ansi、selection **五个模块按位置解构**（`cell.0`、`cell.4`、`cell.7`、`cell.9`…）。这不是一个类型，是一张没有名字的表。

后果：

- 加一个属性要改 5 个模块的全部解构点，编译器只帮到「元数不对」，帮不到「位置错」；
- 颜色是**字符串约定**（`"pN"` / `"#rrggbb"` / `"default"`），于是长出了**三个互不相同的颜色解析器**：`color_attr_to_string`（term.rs:71）、`resolve_color`（render/common.rs:61）、`parse_color`（surface_render.rs:22）。三者接受的语法不同、失败行为也不同（`parse_color` 静默回落 Default，`resolve_color` 返回 None）；
- `cells_of_line`（term.rs:90）与 `render::visible_lines`（render/mod.rs:31）是同一逻辑两份；
- 每格 3 个 `String` 堆分配，正好落在每帧路径上。

**建议**：`struct Cell { col: usize, text: ..., fg: Color, bg: Color, attrs: Attrs, width: u8 }`，`Color` 用枚举（`Copy`）。所有消费者共用；颜色解析只留一处。顺带解决 B4（每帧分配）。

## 4. 三条渲染管线，能力按「类」切分而不是按「阶段」切分

同一份终端模型，有三条互不相干的出图路径：

1. `Terminal.render_ansi` —— 手写 CUP + SGR + `\x1b[K`；
2. `Terminal.render_svg` / `render_image` —— 自研渲染器（fontdb + fontdue + tiny-skia）；
3. `Mux.render` —— `wezterm-surface` 增量 diff + `TerminfoRenderer`。

机制不同不是问题（全量 vs 增量本来就该不同），问题是**能力被绑死在类上**：

- 想要**增量**输出 → 必须用 `Mux`；
- 想要 **SVG / 图片** → 必须用 `Terminal`；
- 想要**两者** → 没有路。`Mux` 的 pane 导不出图，`Terminal` 给不了增量。

而 `Surface` 明明是一个与来源无关的增量层，却没有「把某个终端模型的可见区刷进 Surface」这条公开通路——它只能被 `Mux` 内部用。

**建议**：把管线切成两条正交的轴——`模型 → 网格快照`，以及 `网格 → {ANSI 全量, SVG, 图片}`；增量 diff 作为独立的、两边都能挂的层。这样 `Mux` 的 pane 天然支持导出，`Terminal` 也能接增量。

## 5. 状态所有权：一个 pane 拆成 8–13 个独立 Mutex，且没有「帧」

- `PyTerminal`：`terminal` / `capture` / `view_offset` / `selection` / `mode` / `mode_tail` / 4 个回调 = **10 个锁**。
- `Mux::PaneInner`：再加 `master` / `slave` / `writer` / `child` / `output_buf` / `last_seqno` / `repaint_pending` / `reader_thread` / `rect` ≈ **13 个锁**。

三个后果：

- **锁序靠约定**：`terminal → writer`（A7 的阻塞写）、`terminal → view_offset`，没有任何地方集中声明顺序，也没有 `Mutex` 分组。178 处 `lock().unwrap()` 让任何一次持锁 panic 变成永久毒化（A14）。
- **帧内不原子**：`Mux.render()`（mux.rs:1104-1120）是「取 A 的锁 → 合成 A → 取 B 的锁 → 合成 B」。两个 pane 是在**不同时刻**取的，屏幕上可以同时呈现两个不同瞬间的画面。对复用器这是可观察的撕裂，而且没有帧序号可以对齐。
- **权威与派生混在一起**：`terminal` 是权威模型，`mode` 是**派生状态**（问题 2 里那套重复解析的结果），`view_offset`/`selection` 是视图/交互状态。三者平铺在同一层，看不出谁说了算。

**建议**：按 `Model / View / Interaction` 分组；派生状态要么删掉（问题 2），要么标注来源。引入帧序号：`render()` 先一次性采集各 pane 的快照（或各自带 seqno），再合成，保证一帧一个时刻。

## 6. `pane_id` 是 Vec 下标，不是身份

`Mux::close_pane` 用 `st.panes.remove(pane_id)`（mux.rs:1373），之后 `panes[1]` 就变成了 `panes[0]`。宿主之前缓存的 `b = 1` 现在指向**另一个 pane**。所有 `pane_*` API 都吃这个下标。

这是典型的 index-as-identity。**`slotmap` 已经在依赖树里**（本次构建日志里有 `slotmap v1.1.1`）——用世代索引（或单调 id + `HashMap`）就能让 id 永远有效，关闭后查询返回明确的「不存在」而不是打到别人身上。

## 7. 生命周期靠裸 `usize` 跨语言传递

`spawn` 返回 `(pid, handle)`；`hpcon()` / `child_handle()` 返回裸句柄；`job_handle` 收裸句柄。所有权全靠文档约定（「句柄由调用方持有并负责关闭」）。Python 侧无法检查——用错就是 use-after-free 或句柄泄漏，而且**返回的进程句柄其实归 `Pty` 内部所有**，文档没说清楚谁不能关。

**建议**：既然已经有 `Pty` 对象可以承载所有权，就返回不透明包装（`ProcessHandle` / `Hpcon` / `JobObject`），`Drop` 管理生命周期，要整数时显式 `.raw()`。这类 API 的形状应该让「用错」写不出来。

## 8. 平台分支散落在业务模块，没有平台层

`#[cfg(windows)]` 分布在 pty / mux / clipboard / console_input / lib 五个模块，而且混着两种完全不同的语义：

- 「Windows 才有这个能力」：job object、HPCON、console input；
- 「非 Windows 先占位」：`clipboard_read` 返回空串、`child_handle` 返回 0、`cancel_reader_thread` 不存在。

于是非 Windows 路径实质上是「能编译」而不是「被设计过」。`Pty.spawn` 在 Linux 上**根本拿不到进程句柄**——这是设计缺口，不是平台限制（pidfd / `/proc` 都有）。

**建议**：把「取消阻塞读」「侧载宿主目录」「进程句柄」抽成 `platform` 模块里的同名接口，各平台各自实现（Linux 用 `SA_RESTART` 关闭 + `pthread_kill`，句柄用 pidfd）。业务代码只调接口，`cfg` 不再出现在业务模块里。

## 9. 自己的资源位置从「宿主的状态」推导

- `ensure_conpty_dir`（pty.rs:58）从 `pywezterm.__file__` 反推包目录；
- `font.rs:20` 从**进程 CWD** 找 `src/assets/fonts`。

两者都在问别人「我是谁」，而不是问自己。正确来源是扩展模块**自身的磁盘位置**（Windows `GetModuleFileNameW` 拿到 `.pyd` 路径，POSIX 用 `dladdr`）。CWD 那条已经彻底失效（D8）；`__file__` 那条能用，但把「模块何时被 import、`__file__` 何时被设置」变成了隐式前置条件——注释里也承认 pymodule init 阶段拿不到，只能推迟到第一次建 `Pty`。

同时整条链路上「侧载 conpty 到底生效没有」**没有任何可观测信号**（找不到就静默回落系统 conhost）。这和 `buffered_bytes()` 那种「把不变量暴露出来」的做法正好相反。

**建议**：模块加载时用自己的路径算一次存进 `OnceLock`；对外给一个 `pywezterm.env_info()`（sidecar 路径、是否加载、字体来源、CWD），让这些隐式前提变成可查的事实。

## 10. 错误模型三种风格混用

- `PyResult` + `PyRuntimeError::new_err(format!("中文串"))`；
- 静默成功（`clipboard_write` 打开失败返回 `Ok(())`；close 后 `write` 返回 `Ok(())`；`read` 返回 `b""`）；
- `panic` / `expect`（`render_image` 的 scale、零 pane 的 `render`）。

宿主无法按**类型**区分「可重试的瞬时错误」「状态错误（已关闭）」「编程错误（pane 不存在）」，只能去匹配中文字符串——而错误串里又混着 wezterm 的英文 `{e:#}`。

**建议**：一个 `enum Error { Pty(io::Error), Closed, NoSuchPane(usize), Render(String) }` → 映射到 `PyWeztermError` 的子类。顺带把「静默吞掉」的场合明确成文档契约（哪些是 best-effort）。

## 11. 测试架构缺了「分块边界」这一类不变量

现有测试的形态一律是「喂一小段 → 查状态」。而 A2、A3 **都是分块边界 bug**——同一段字节流按不同方式切分喂入，结果应当完全相同。

这类不变量应该被写成 property：

> 对同一段输入，按 1 / 7 / 63 / 64 / 65 / 4096 字节切分喂入，
> 断言 `text()` / `snapshot()` / **模式状态** / 输出录制**逐字节一致**。

这一个形态能同时抓住 A2、A3 和未来所有同类问题；而且问题 1 的两套实现正好需要同一套断言（这正是发现「两套已经分叉」的机制）。

**建议**：把 `tests/` 从「API 用例集」补成「不变量集」：分块不变性、幂等性（`close()` 两次、`resize` 同尺寸）、以及失败路径（`add_pane` spawn 失败后 Mux 仍可用）。

## 12. 单一事实来源缺失（文档 / 资源 / 契约）

- `docs/PYWEZTERM_API.zh.md` 与 `.en.md` 手工各维护 416 行，没有生成、没有 CI 校验 → 必然漂移；
- README、docs、`tests/` 三处各写一遍用法；
- conpty 二进制的路径约定散在 `build.rs`、`pyproject.toml`、`pty.rs`、`AGENTS.md` **四处**，靠大段注释互相提醒（「不要写成 `conpty/*`，曾如此」）——一个跨两个构建工具的隐式契约，靠注释维系；
- `AGENTS.md` 的变更记录和代码里的对应注释是两份。

**建议**：API 文档从 docstring 生成（或至少 CI 比对）；构建产物放置收成一个显式打包步骤，别让 `build.rs` 和 maturin `include` 隔空约定。

---

## 如果只动三刀

1. **用「改 vendored wezterm 暴露模式状态」替换手写解析器**（问题 2）。
   删约 200 行，消灭一整类 bug（A2），并让 `Mux` 白捡这个能力。改动最小、收益最大。
2. **把 `CellTuple` 换成具名 `Cell` + `Color` 枚举**（问题 3）。
   收敛 IR，顺带消掉三个颜色解析器、两处 cells 构造重复、以及每帧的逐格 String 分配（B4）。
3. **抽 `Pane` 统一宿主单元**（问题 1）。
   消掉双实现与分叉，让背压/模式跟踪/导出对 `Mux` 同时生效。这是最大的一刀。

**顺序很重要**：动 1 和 3 之前，先把问题 11 的「分块不变量」测试补上——否则大改没有安全网，而 A2/A3 恰恰证明这类 bug 不会被现有测试发现。
