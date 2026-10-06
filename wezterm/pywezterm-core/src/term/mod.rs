//! 终端领域层：模型、视口、选区、输入编码、网格。
//!
//! 输入侧的 VT 解析只有一份 —— `wezterm_term`。本层不自己解析字节流，只做转发与查询；
//! 唯一的例外是**模式设置序列的观察记录**（[`model::Model`]），因为模型没有暴露
//! 「应用显式设置过哪些模式」这一信息，而订阅方恢复客户端状态需要它。

pub mod encode;
pub mod grid;
pub mod model;
pub mod selection;
pub mod view;

pub use grid::{cells_of_line, Attrs, Cell, Color};
pub use model::Model;
pub use selection::{Selection, SelectionKind};
pub use view::View;
