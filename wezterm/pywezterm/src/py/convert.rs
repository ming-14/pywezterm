//! 领域类型 → Python 对象。
//!
//! 颜色与网格是**枚举/结构体**，字符串形式只在这里出现 —— 边界即转换点。

use pyo3::prelude::*;

use pywezterm_core::term::grid::Cell;

/// 单元格元组：`(col, ch, fg, bg, bold, italic, underline, reverse, strike, width)`。
pub type CellTuple = (usize, String, String, String, bool, bool, bool, bool, bool, u8);

/// 单元格 → 元组。
pub fn cell(c: &Cell) -> CellTuple {
    (
        c.col,
        c.text.clone(),
        c.fg.to_python(),
        c.bg.to_python(),
        c.attrs.bold,
        c.attrs.italic,
        c.attrs.underline,
        c.attrs.reverse,
        c.attrs.strikethrough,
        c.width,
    )
}

/// 多行单元格 → Python 列表的列表。
pub fn rows(py: Python<'_>, lines: &[Vec<Cell>]) -> PyResult<Py<PyAny>> {
    let items: Vec<Vec<CellTuple>> = lines
        .iter()
        .map(|line| line.iter().map(cell).collect())
        .collect();
    Ok(items.into_pyobject(py)?.into_any().unbind())
}

/// `(是否折行结尾, 单元格)` 列表 → Python 列表。
pub fn wrapped_rows(py: Python<'_>, lines: &[(bool, Vec<Cell>)]) -> PyResult<Py<PyAny>> {
    let items: Vec<(bool, Vec<CellTuple>)> = lines
        .iter()
        .map(|(wrapped, cells)| (*wrapped, cells.iter().map(cell).collect()))
        .collect();
    Ok(items.into_pyobject(py)?.into_any().unbind())
}

/// 逻辑行 `(首 stable, 末 stable, 单元格)` 列表 → Python 列表。
pub fn logical_rows(
    py: Python<'_>,
    lines: &[(isize, isize, Vec<Cell>)],
) -> PyResult<Py<PyAny>> {
    let items: Vec<(isize, isize, Vec<CellTuple>)> = lines
        .iter()
        .map(|(first, last, cells)| (*first, *last, cells.iter().map(cell).collect()))
        .collect();
    Ok(items.into_pyobject(py)?.into_any().unbind())
}
