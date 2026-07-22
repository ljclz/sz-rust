//! PDF 表单填充模块 — 对齐 PHP `mikehaertl\pdftk\Pdf`
//!
//! ## PHP 对齐
//!
//! 本模块以 PHP 项目实际使用的 `mikehaertl/php-pdftk` API 子集为对齐基准：
//!
//! - `new Pdf($path)` → [`Pdf::load`]
//! - `$pdf->fillForm($data)` → [`Pdf::fill_form`]
//! - `$pdf->flatten()` → [`Pdf::flatten`]
//! - `$pdf->saveAs($url)` → [`Pdf::save_as`]
//! - `$pdf->send($filename)` → [`Pdf::to_bytes`]（输出到内存缓冲，由调用方负责 HTTP 响应）
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\app\oapi\controller\Index.php`（`outpdf` 方法）
//!
//! ```php
//! $pdf = new Pdf($path);
//! $res = $pdf->fillForm($data)->flatten()->saveAs($url);
//! ```
//!
//! ## 实现说明
//!
//! `mikehaertl/php-pdftk` 实际是调用外部 `pdftk` 命令的二进制封装。Rust 端使用
//! 纯 Rust 的 `lopdf` 库直接操作 PDF 对象：
//!
//! - 填充表单：遍历 `/AcroForm/Fields`，匹配字段名 `/T`，设置值 `/V`
//! - flatten：设置 `/AcroForm/NeedAppearances = true`，让 PDF 阅读器重新生成外观流
//!   （对齐 PHP `FPDM` 的实现方式，非真正烧入页面内容流）
//!
//! ## R5 硬约束
//!
//! - R5-38：`Pdf::load` 加载 PDF 文件
//! - R5-39：`fill_form` 填充 AcroForm 字段
//! - R5-40：`flatten` 设置 `NeedAppearances`
//! - R5-41：`save_as` / `to_bytes` 输出 PDF

use std::collections::HashMap;
use std::path::Path;

use lopdf::{Document, Object, ObjectId};

use crate::PdfError;

// ============================================================================
// Pdf — 对齐 PHP `mikehaertl\pdftk\Pdf`
// ============================================================================

/// PDF 文档（对齐 PHP `mikehaertl\pdftk\Pdf`）
///
/// 使用 builder pattern 实现链式调用，对齐 PHP 的链式 API：
/// `fillForm($data)->flatten()->saveAs($url)`。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_pdf::pdf_form::Pdf;
///
/// let mut data = HashMap::new();
/// data.insert("store_name".to_string(), "太平店".to_string());
/// data.insert("amount".to_string(), "2500".to_string());
///
/// Pdf::load("/path/to/template.pdf")?
///     .fill_form(&data)?
///     .flatten()?
///     .save_as("/path/to/output.pdf")?;
/// ```
pub struct Pdf {
    doc: Document,
    /// 已填充的字段计数（用于诊断）
    filled_count: usize,
    /// 是否已 flatten
    flattened: bool,
}

impl Pdf {
    /// 加载 PDF 文件 — 对齐 PHP `new Pdf($path)`
    ///
    /// # R5-38 硬约束
    ///
    /// 加载失败返回 [`PdfError::Lopdf`] 或 [`PdfError::Io`]。
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, PdfError> {
        let doc = Document::load(path)?;
        Ok(Self {
            doc,
            filled_count: 0,
            flattened: false,
        })
    }

    /// 从内存加载 PDF — 对齐 PHP `new Pdf($content)`（PHP 也支持字符串入参）
    pub fn load_mem(buffer: &[u8]) -> Result<Self, PdfError> {
        let doc = Document::load_mem(buffer)?;
        Ok(Self {
            doc,
            filled_count: 0,
            flattened: false,
        })
    }

    /// 填充表单字段 — 对齐 PHP `$pdf->fillForm($data)`
    ///
    /// # PHP 行为
    ///
    /// PHP `fillForm($data)` 接收关联数组，键为字段名，值为字段值。
    /// 内部通过 pdftk 命令的 `fill_form` 操作完成。
    ///
    /// # Rust 实现
    ///
    /// 遍历 `/AcroForm/Fields` 数组，对每个字段：
    /// 1. 获取字段名 `/T`
    /// 2. 如果字段名匹配 `data` 中的键，设置字段值 `/V`
    ///
    /// # R5-39 硬约束
    ///
    /// - 字段名匹配：精确匹配（对齐 PHP 行为）
    /// - 字段值类型：统一为 PDF String（对齐 PHP pdftk 行为）
    /// - 未匹配的字段：忽略（对齐 PHP 行为）
    pub fn fill_form(mut self, data: &HashMap<String, String>) -> Result<Self, PdfError> {
        // 获取 AcroForm 字段引用
        let acroform_id = self.get_acroform_id()?;
        let field_ids = self.get_field_ids(acroform_id)?;

        let mut filled = 0usize;
        for field_id in field_ids {
            // 获取字段名 /T
            let field_name = match self.get_field_name(field_id)? {
                Some(name) => name,
                None => continue,
            };

            // 匹配 data
            if let Some(value) = data.get(&field_name) {
                self.set_field_value(field_id, value)?;
                filled += 1;
            }
        }

        self.filled_count = filled;
        Ok(self)
    }

    /// 平展表单 — 对齐 PHP `$pdf->flatten()`
    ///
    /// # PHP 行为
    ///
    /// PHP `flatten()` 通过 pdftk 的 `flatten` 操作将表单字段烧入页面内容流，
    /// 使其不可再编辑。
    ///
    /// # Rust 实现
    ///
    /// 真正的 flatten 需要将字段外观流烧入页面内容流，操作复杂。
    /// 此处采用 `NeedAppearances=true` 方案（对齐 PHP `FPDM` 实现方式），
    /// 让 PDF 阅读器在打开时重新生成外观流。
    ///
    /// # R5-40 硬约束
    ///
    /// 设置 `/AcroForm/NeedAppearances = true`。
    pub fn flatten(mut self) -> Result<Self, PdfError> {
        let acroform_id = self.get_acroform_id()?;
        let acroform_dict = self
            .doc
            .get_dictionary_mut(acroform_id)
            .map_err(|e| PdfError::Pdf(format!("get AcroForm dict failed: {}", e)))?;
        acroform_dict.set("NeedAppearances", Object::Boolean(true));
        self.flattened = true;
        Ok(self)
    }

    /// 保存到文件 — 对齐 PHP `$pdf->saveAs($url)`
    ///
    /// # R5-41 硬约束
    ///
    /// 保存失败返回 [`PdfError::Io`]。
    pub fn save_as<P: AsRef<Path>>(mut self, path: P) -> Result<(), PdfError> {
        self.doc.save(path)?;
        Ok(())
    }

    /// 输出到内存缓冲 — 对齐 PHP `$pdf->send($filename)`
    ///
    /// PHP `send()` 输出 PDF 到浏览器下载。Rust 端返回字节缓冲，
    /// 由调用方负责 HTTP 响应（设置 Content-Type、Content-Disposition 等头）。
    ///
    /// # R5-41 硬约束
    pub fn to_bytes(mut self) -> Result<Vec<u8>, PdfError> {
        let mut buffer = Vec::new();
        self.doc.save_to(&mut buffer)?;
        Ok(buffer)
    }

    /// 获取已填充字段数（用于诊断）
    pub fn filled_count(&self) -> usize {
        self.filled_count
    }

    /// 是否已 flatten
    pub fn is_flattened(&self) -> bool {
        self.flattened
    }

    // ------------------------------------------------------------------------
    // 内部辅助方法
    // ------------------------------------------------------------------------

    /// 获取 AcroForm 字典的 ObjectId
    ///
    /// PDF 结构：`Catalog -> AcroForm`
    fn get_acroform_id(&self) -> Result<ObjectId, PdfError> {
        let catalog = self
            .doc
            .catalog()
            .map_err(|e| PdfError::Pdf(format!("get catalog failed: {}", e)))?;
        let acroform_obj = catalog
            .get(b"AcroForm")
            .map_err(|_| PdfError::Pdf("PDF has no AcroForm (not a form PDF)".to_string()))?;
        self.resolve_ref(acroform_obj)
    }

    /// 获取 AcroForm 下所有字段 的 ObjectId 列表
    ///
    /// PDF 结构：`AcroForm -> Fields -> [Field1, Field2, ...]`
    fn get_field_ids(&self, acroform_id: ObjectId) -> Result<Vec<ObjectId>, PdfError> {
        let acroform_dict = self
            .doc
            .get_dictionary(acroform_id)
            .map_err(|e| PdfError::Pdf(format!("get AcroForm dict failed: {}", e)))?;
        let fields_obj = acroform_dict
            .get(b"Fields")
            .map_err(|_| PdfError::Pdf("AcroForm has no Fields array".to_string()))?;

        match fields_obj {
            Object::Array(arr) => {
                let mut ids = Vec::with_capacity(arr.len());
                for item in arr {
                    if let Object::Reference(id) = item {
                        ids.push(*id);
                    } else {
                        return Err(PdfError::Pdf(format!(
                            "Fields array item is not a Reference: {:?}",
                            item
                        )));
                    }
                }
                Ok(ids)
            }
            _ => Err(PdfError::Pdf("AcroForm/Fields is not an Array".to_string())),
        }
    }

    /// 获取字段名 `/T`
    ///
    /// PDF 结构：`Field -> T (field name string)`
    fn get_field_name(&self, field_id: ObjectId) -> Result<Option<String>, PdfError> {
        let field_dict = self
            .doc
            .get_dictionary(field_id)
            .map_err(|e| PdfError::Pdf(format!("get field dict failed: {}", e)))?;
        match field_dict.get(b"T") {
            Ok(Object::String(bytes, _)) => {
                let name = String::from_utf8_lossy(bytes).into_owned();
                Ok(Some(name))
            }
            Ok(Object::Name(name_bytes)) => {
                let name = String::from_utf8_lossy(name_bytes).into_owned();
                Ok(Some(name))
            }
            Ok(other) => Err(PdfError::Pdf(format!("unexpected /T type: {:?}", other))),
            Err(_) => Ok(None), // 字段无 /T，可能是子字段容器
        }
    }

    /// 设置字段值 `/V`
    ///
    /// PDF 结构：`Field -> V (field value)`
    ///
    /// 对齐 PHP `pdftk`：所有值统一转为 PDF String。
    fn set_field_value(&mut self, field_id: ObjectId, value: &str) -> Result<(), PdfError> {
        let field_dict = self
            .doc
            .get_dictionary_mut(field_id)
            .map_err(|e| PdfError::Pdf(format!("get field dict mut failed: {}", e)))?;
        // 对齐 pdftk：字符串值使用 PDF String 对象
        field_dict.set(
            "V",
            Object::String(value.as_bytes().to_vec(), lopdf::StringFormat::Literal),
        );
        Ok(())
    }

    /// 解引用 Reference 对象，返回 ObjectId
    fn resolve_ref(&self, obj: &Object) -> Result<ObjectId, PdfError> {
        match obj {
            Object::Reference(id) => Ok(*id),
            _ => Err(PdfError::Pdf(format!("expected Reference, got {:?}", obj))),
        }
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;

    /// 构造一个最小的 AcroForm PDF 用于测试
    ///
    /// PDF 结构：
    /// - Catalog
    ///   - AcroForm (Dictionary)
    ///     - Fields (Array)
    ///       - Field1: T="store_name", V=""
    ///       - Field2: T="amount", V=""
    ///       - Field3: T="year", V=""
    fn make_test_pdf() -> tempfile::NamedTempFile {
        let mut doc = Document::new();

        // 创建 Pages 字典（必需，PDF 结构要求）
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(Vec::new()));
        let pages_id = doc.add_object(pages_dict);

        // 创建 Catalog 字典
        let mut catalog_dict = Dictionary::new();
        catalog_dict.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog_dict.set("Pages", Object::Reference(pages_id));

        // 创建字段字典的辅助闭包
        let make_field = |name: &str| -> Dictionary {
            let mut dict = Dictionary::new();
            dict.set(
                "T",
                Object::String(name.as_bytes().to_vec(), lopdf::StringFormat::Literal),
            );
            dict.set(
                "V",
                Object::String(Vec::new(), lopdf::StringFormat::Literal),
            );
            dict
        };

        let field1_id = doc.add_object(make_field("store_name"));
        let field2_id = doc.add_object(make_field("amount"));
        let field3_id = doc.add_object(make_field("year"));

        // 创建 AcroForm
        let mut acroform = Dictionary::new();
        acroform.set(
            "Fields",
            Object::Array(vec![
                Object::Reference(field1_id),
                Object::Reference(field2_id),
                Object::Reference(field3_id),
            ]),
        );
        acroform.set("NeedAppearances", Object::Boolean(false));
        let acroform_id = doc.add_object(acroform);

        catalog_dict.set("AcroForm", Object::Reference(acroform_id));
        let catalog_id = doc.add_object(catalog_dict);

        // 设置 trailer 的 Root
        doc.trailer.set("Root", Object::Reference(catalog_id));

        // 保存到临时文件
        let tmp = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();
        doc.save(tmp.path()).unwrap();
        tmp
    }

    // ------------------------------------------------------------------------
    // R5-38：Pdf::load 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_38_load_pdf() {
        let tmp = make_test_pdf();
        let result = Pdf::load(tmp.path());
        assert!(result.is_ok());
        let pdf = result.unwrap();
        assert_eq!(pdf.filled_count(), 0);
        assert!(!pdf.is_flattened());
    }

    #[test]
    fn test_r5_38_load_nonexistent() {
        let result = Pdf::load("/nonexistent/path/file.pdf");
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------------
    // R5-39：fill_form 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_39_fill_form_basic() {
        let tmp = make_test_pdf();

        let mut data = HashMap::new();
        data.insert("store_name".to_string(), "太平店".to_string());
        data.insert("amount".to_string(), "2500".to_string());

        let pdf = Pdf::load(tmp.path()).unwrap().fill_form(&data).unwrap();

        assert_eq!(pdf.filled_count(), 2);
    }

    #[test]
    fn test_r5_39_fill_form_partial_match() {
        let tmp = make_test_pdf();

        let mut data = HashMap::new();
        data.insert("store_name".to_string(), "太平店".to_string());
        // year 不填，amount 不填
        // 多余字段（PDF 中不存在）
        data.insert("nonexistent_field".to_string(), "value".to_string());

        let pdf = Pdf::load(tmp.path()).unwrap().fill_form(&data).unwrap();

        // 只匹配 store_name
        assert_eq!(pdf.filled_count(), 1);
    }

    #[test]
    fn test_r5_39_fill_form_empty_data() {
        let tmp = make_test_pdf();
        let data = HashMap::new();
        let pdf = Pdf::load(tmp.path()).unwrap().fill_form(&data).unwrap();
        assert_eq!(pdf.filled_count(), 0);
    }

    #[test]
    fn test_r5_39_fill_form_no_acroform() {
        // 构造无 AcroForm 的 PDF
        let mut doc = Document::new();
        // 创建必要的 Catalog/Pages 结构，但不添加 AcroForm
        let mut pages_dict = Dictionary::new();
        pages_dict.set("Count", Object::Integer(0));
        pages_dict.set("Kids", Object::Array(Vec::new()));
        let pages_id = doc.add_object(pages_dict);
        let mut catalog_dict = Dictionary::new();
        catalog_dict.set("Type", Object::Name(b"Catalog".to_vec()));
        catalog_dict.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(catalog_dict);
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let tmp = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();
        doc.save(tmp.path()).unwrap();

        let mut data = HashMap::new();
        data.insert("store_name".to_string(), "value".to_string());

        let result = Pdf::load(tmp.path()).unwrap().fill_form(&data);
        assert!(result.is_err());
        assert!(result
            .err()
            .map(|e| matches!(e, PdfError::Pdf(_)))
            .unwrap_or(false));
    }

    // ------------------------------------------------------------------------
    // R5-40：flatten 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_40_flatten() {
        let tmp = make_test_pdf();
        let pdf = Pdf::load(tmp.path()).unwrap().flatten().unwrap();
        assert!(pdf.is_flattened());
    }

    // ------------------------------------------------------------------------
    // R5-41：save_as / to_bytes 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_r5_41_save_as() {
        let tmp_in = make_test_pdf();
        let tmp_out = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();

        let mut data = HashMap::new();
        data.insert("store_name".to_string(), "太平店".to_string());

        let result = Pdf::load(tmp_in.path())
            .unwrap()
            .fill_form(&data)
            .unwrap()
            .flatten()
            .unwrap()
            .save_as(tmp_out.path());
        assert!(result.is_ok());

        // 验证输出文件存在且非空
        let metadata = std::fs::metadata(tmp_out.path()).unwrap();
        assert!(metadata.len() > 0);
    }

    #[test]
    fn test_r5_41_to_bytes() {
        let tmp_in = make_test_pdf();

        let mut data = HashMap::new();
        data.insert("amount".to_string(), "2500".to_string());

        let result = Pdf::load(tmp_in.path())
            .unwrap()
            .fill_form(&data)
            .unwrap()
            .flatten()
            .unwrap()
            .to_bytes();
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert!(!bytes.is_empty());

        // PDF 文件应以 %PDF 开头
        assert!(bytes.starts_with(b"%PDF"));
    }

    // ------------------------------------------------------------------------
    // 业务场景对齐测试 — 对齐 Index::outpdf
    // ------------------------------------------------------------------------

    #[test]
    fn test_business_outpdf_pattern() {
        // 对齐 `app\oapi\controller\Index::outpdf` 方法
        // 业务模式：1. new Pdf($path) 2. fillForm($data) 3. flatten() 4. saveAs($url)
        let tmp_in = make_test_pdf();
        let tmp_out = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();

        // 对齐 PHP 业务数据（截取部分）
        let mut data = HashMap::new();
        data.insert("store_name".to_string(), "太平店".to_string());
        data.insert("iscompany".to_string(), "Yes".to_string());
        data.insert("No".to_string(), "0000658".to_string());
        data.insert("year".to_string(), "2021".to_string());
        data.insert("amount".to_string(), "2500".to_string());

        // PHP: $pdf = new Pdf($path);
        // PHP: $res = $pdf->fillForm($data)->flatten()->saveAs($url);
        let result = Pdf::load(tmp_in.path())
            .unwrap()
            .fill_form(&data)
            .unwrap()
            .flatten()
            .unwrap()
            .save_as(tmp_out.path());

        assert!(result.is_ok());

        // 验证保存后的文件可以再次加载
        let reload = Pdf::load(tmp_out.path());
        assert!(reload.is_ok());
    }

    // ------------------------------------------------------------------------
    // load_mem 测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_load_mem() {
        let tmp = make_test_pdf();
        let bytes = std::fs::read(tmp.path()).unwrap();
        let result = Pdf::load_mem(&bytes);
        assert!(result.is_ok());
    }

    // ------------------------------------------------------------------------
    // 链式调用顺序测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_chain_fill_then_flatten_then_save() {
        let tmp_in = make_test_pdf();
        let tmp_out = tempfile::Builder::new().suffix(".pdf").tempfile().unwrap();

        let mut data = HashMap::new();
        data.insert("store_name".to_string(), "测试".to_string());

        // 验证链式调用顺序：fill_form -> flatten -> save_as
        let pdf = Pdf::load(tmp_in.path()).unwrap();
        assert_eq!(pdf.filled_count(), 0);
        assert!(!pdf.is_flattened());

        let pdf = pdf.fill_form(&data).unwrap();
        assert_eq!(pdf.filled_count(), 1);
        assert!(!pdf.is_flattened());

        let pdf = pdf.flatten().unwrap();
        assert_eq!(pdf.filled_count(), 1); // flatten 不改变 filled_count
        assert!(pdf.is_flattened());

        let result = pdf.save_as(tmp_out.path());
        assert!(result.is_ok());
    }

    // ------------------------------------------------------------------------
    // 辅助方法测试
    // ------------------------------------------------------------------------

    #[test]
    fn test_filled_count_and_flattened() {
        let tmp = make_test_pdf();
        let pdf = Pdf::load(tmp.path()).unwrap();
        assert_eq!(pdf.filled_count(), 0);
        assert!(!pdf.is_flattened());
    }
}
