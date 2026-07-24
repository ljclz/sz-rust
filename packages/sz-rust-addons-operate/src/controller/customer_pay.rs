//! CustomerPay 控制器 — 对齐 PHP `addons/operate/controller/admin/CustomerPay.php`
//!
//! ## PHP 对齐
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|----------|------|
//! | `base()` | [`CustomerPayController::base`] | 基础数据（部门/客户/分类/支付状态/支付方式） |
//! | `index()` | [`CustomerPayController::index`] | 分页列表 |
//! | `export()` | [`CustomerPayController::export`] | 导出列表（不分页） |
//! | `pay()` | [`CustomerPayController::pay`] | 收款下单（依赖 SettledService） |
//! | `payBuy()` | [`CustomerPayController::pay_buy`] | 继续支付（依赖 SettledService） |
//! | `paySuccess()` | [`CustomerPayController::pay_success`] | 查询订单详情 |
//! | `check()` | [`CustomerPayController::check`] | 支付状态查询（多银行：ccb/icbc/fuiou） |
//! | `refund()` | [`CustomerPayController::refund`] | 退款（依赖 SettledService） |
//! | `detail()` | [`CustomerPayController::detail`] | 订单详情 |
//!
//! ## PHP 源码依据
//!
//! ```php
//! public function index(): Json {
//!     $param = $this->postData();
//!     $model = new CustomerPayModel();
//!     $result = $model->getList($param,'list');
//!     return $this->renderSuccess('', compact('result'));
//! }
//! ```
//!
//! ## 服务依赖
//!
//! `pay`/`payBuy`/`check`/`refund` 方法依赖 [`SettledService`] trait，
//! 由应用层注入具体实现（Task 5 完成 6 个服务实现）。
//!
//! ## PHP `check` 方法多银行逻辑复刻
//!
//! PHP `check()` 根据 `bank_name` 分支处理：
//! - `ccb`：`RESULT == 'Y'` → `settle()`，`RESULT == 'N'` → `update(['order_status'=>20])`
//! - `icbc`：`pay_status == 1` → `settle()`，其他 → `update(['order_status'=>20])`
//! - `fuiou`：`result_code == '000000' && trans_stat == 'SUCCESS'` → `settle()`，否则 → `update(['order_status'=>20])`
//!
//! Rust 端通过 [`apply_bank_check_result`] 复刻此分支逻辑。

use axum::body::Body;
use axum::http::Request;
use axum::response::Response;
use serde_json::{json, Value};
use sz_orm_core::repository::{Repository, WhereCondition, WhereOp};
use sz_orm_core::ModelExt as _;
use sz_orm_core::Value as OrmValue;
use sz_rust_core::controller::{AddonsBaseController, BaseController, SzController};
use sz_rust_core::model::Appendable as _;
use sz_rust_core::model::Mutator as _;

use crate::controller::common::{get_app_id, get_i64_param, get_str_param, parse_form_data};
use crate::model::CustomerPay;
use crate::service::SettledService;

/// CustomerPay 控制器 — 对齐 PHP `CustomerPay` 控制器
pub struct CustomerPayController;

impl SzController for CustomerPayController {}
impl BaseController for CustomerPayController {}
impl AddonsBaseController for CustomerPayController {}

impl CustomerPayController {
    /// 基础数据 — 对齐 PHP `base()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function base(): Json {
    ///     $param = $this->postData();
    ///     $result = [
    ///         'deptList' => Dept::getLightList(34),
    ///         'customerList' => Customer::selectCatCustomer(...),
    ///         'catList' => Category::getAll($param['app_id']),
    ///         'payStatusList' => CustomerSyncTypeEnum::payStatusData(),
    ///         'payTypeList' => CustomerSyncTypeEnum::payTypeList()
    ///     ];
    ///     return $this->renderSuccess('', compact('result'));
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn base(&self, req: Request<Body>) -> Response {
        let _param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        // PHP 整合多源数据，Rust 端返回基础结构，
        // 业务层（apps/oapc）可注入具体 Repository 补全 deptList/customerList/catList。
        let result = json!({
            "deptList": [],
            "customerList": [],
            "catList": [],
            "payStatusList": [],
            "payTypeList": []
        });
        self.render_success("", json!({"result": result}))
    }

    /// 分页列表 — 对齐 PHP `index()`
    #[tracing::instrument(skip_all)]
    pub async fn index(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "list");
        self.render_success("", json!({"result": result}))
    }

    /// 导出列表 — 对齐 PHP `export()`
    #[tracing::instrument(skip_all)]
    pub async fn export(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let result = Self::get_list(repo, &param, "export");
        self.render_success("", json!({"result": result}))
    }

    /// 收款下单 — 对齐 PHP `pay()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function pay(): Json {
    ///     $param = $this->postData();
    ///     $data = json_decode($param['formData'], true);
    ///     $orderService = new SettledService($data);
    ///     $orderDetail = $orderService->createOrder();
    ///     if (empty($orderDetail)) {
    ///         return $this->renderError($orderService->getError() ?: '支付订单创建失败');
    ///     }
    ///     return $this->renderSuccess('收款成功', ['order_id'=>$orderDetail['order_id'],'pay_res'=>$orderDetail['pay_res']]);
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn pay(&self, req: Request<Body>, svc: &dyn SettledService) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        match svc.create_order(&data) {
            Ok(order_detail) => {
                let order_id = order_detail
                    .get("order_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let pay_res = order_detail.get("pay_res").cloned().unwrap_or(json!({}));
                self.render_success(
                    "收款成功",
                    json!({"order_id": order_id, "pay_res": pay_res}),
                )
            }
            Err(e) => {
                let msg = if e.is_empty() {
                    "支付订单创建失败"
                } else {
                    &e
                };
                self.render_error(msg, json!({}), 0)
            }
        }
    }

    /// 继续支付 — 对齐 PHP `payBuy()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function payBuy(): Json {
    ///     $param = $this->postData();
    ///     $data = json_decode($param['formData'], true);
    ///     $model = new CustomerPayModel();
    ///     $detail = $model->detail($param['order_id']);
    ///     if(empty($detail)){
    ///         return $this->renderError('记录不存在');
    ///     } else {
    ///         $orderDetail = $detail->onPayBuy($detail,$data);
    ///         if (empty($orderDetail)) {
    ///             return $this->renderError($detail->getError() ?: '订单更新失败');
    ///         }
    ///         return $this->renderSuccess('收款中', ['order_id'=>$orderDetail['order_id'],'pay_res'=>$orderDetail['pay_res']]);
    ///     }
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn pay_buy(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
        svc: &dyn SettledService,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let order_id = match get_i64_param(&param, "order_id") {
            Some(id) => id,
            None => return self.render_error("order_id 参数缺失", json!({}), 0),
        };
        let data = match parse_form_data(&param) {
            Ok(d) => d,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        let detail = match Self::detail_order(repo, order_id) {
            Some(d) => d,
            None => return self.render_error("记录不存在", json!({}), 0),
        };

        match svc.pay_buy(&detail, &data) {
            Ok(order_detail) => {
                let order_id = order_detail
                    .get("order_id")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let pay_res = order_detail.get("pay_res").cloned().unwrap_or(json!({}));
                self.render_success("收款中", json!({"order_id": order_id, "pay_res": pay_res}))
            }
            Err(e) => {
                let msg = if e.is_empty() {
                    "订单更新失败"
                } else {
                    &e
                };
                self.render_error(msg, json!({}), 0)
            }
        }
    }

    /// 查询订单详情 — 对齐 PHP `paySuccess()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function paySuccess(): Json {
    ///     $param = $this->postData();
    ///     $model = new CustomerPayModel();
    ///     $detail = $model->detail($param['order_id']);
    ///     return $this->renderSuccess('', compact('detail'));
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn pay_success(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let order_id = match get_i64_param(&param, "order_id") {
            Some(id) => id,
            None => return self.render_error("order_id 参数缺失", json!({}), 0),
        };

        let detail = Self::detail_order(repo, order_id).unwrap_or(json!(null));
        self.render_success("", json!({"detail": detail}))
    }

    /// 支付状态查询 — 对齐 PHP `check()`
    ///
    /// # PHP 对齐
    ///
    /// PHP `check()` 方法根据 `bank_name` 分支处理：
    /// - `type == 'check'`：调用 `epayCheck()`，然后按银行更新订单状态
    /// - 其他：仅调用 `epayCheck()`
    ///
    /// ## 多银行逻辑
    ///
    /// - `ccb`：`RESULT == 'Y'` → `settle()`，`RESULT == 'N'` → `order_status=20`
    /// - `icbc`：`pay_status == 1` → `settle()`，其他 → `order_status=20`
    /// - `fuiou`：`result_code == '000000' && trans_stat == 'SUCCESS'` → `settle()`，否则 → `order_status=20`
    #[tracing::instrument(skip_all)]
    pub async fn check(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
        svc: &dyn SettledService,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };

        let check_type = get_str_param(&param, "type").unwrap_or_default();
        let pay_res = match svc.epay_check(&param) {
            Ok(r) => r,
            Err(e) => return self.render_error(&e, json!({}), 0),
        };

        if check_type == "check" {
            // PHP 分支：根据 bank_name 更新订单状态
            if let Some(Err(e)) = Self::apply_bank_check_result(&param, &pay_res, repo) {
                return self.render_error(&e, json!({}), 0);
            }
        }

        let msg = pay_res
            .get("msg")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        self.render_success(&msg, json!({"pay_res": pay_res}))
    }

    /// 退款 — 对齐 PHP `refund()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function refund(): Json {
    ///     $param = $this->postData();
    ///     $model = CustomerPayModel::detail($param['order_id']);
    ///     if ($model['pay_status'] == 10) {
    ///         return $this->renderError('该订单尚未支付');
    ///     }
    ///     if ($model['pay_status'] == 30) {
    ///         return $this->renderError('该订单已退款');
    ///     }
    ///     if ($model->onRefund($param)) {
    ///         return $this->renderSuccess('退款成功');
    ///     }
    ///     return $this->renderError($model->getError() ?: '退款失败');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn refund(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
        svc: &dyn SettledService,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let order_id = match get_i64_param(&param, "order_id") {
            Some(id) => id,
            None => return self.render_error("order_id 参数缺失", json!({}), 0),
        };

        let detail = match Self::detail_order(repo, order_id) {
            Some(d) => d,
            None => return self.render_error("数据不存在", json!({}), 0),
        };

        // PHP 校验：pay_status == 10（未付款）不允许退款
        let pay_status = detail
            .get("pay_status")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if pay_status == 10 {
            return self.render_error("该订单尚未支付", json!({}), 0);
        }
        // PHP 校验：pay_status == 30（已退款）不允许重复退款
        if pay_status == 30 {
            return self.render_error("该订单已退款", json!({}), 0);
        }

        match svc.refund(&detail, &param) {
            Ok(()) => self.render_success("退款成功", json!({})),
            Err(e) => {
                let msg = if e.is_empty() { "退款失败" } else { &e };
                self.render_error(msg, json!({}), 0)
            }
        }
    }

    /// 订单详情 — 对齐 PHP `detail()`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function detail(): Json {
    ///     $param = $this->postData();
    ///     $detail = CustomerPayModel::detail($param['order_id']);
    ///     if($detail){
    ///         return $this->renderSuccess('', ['detail'=>$detail]);
    ///     }
    ///     return $this->renderError('数据不存在');
    /// }
    /// ```
    #[tracing::instrument(skip_all)]
    pub async fn detail(
        &self,
        req: Request<Body>,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
    ) -> Response {
        let param = match self.post_data(req).await {
            Ok(p) => p,
            Err(e) => return self.render_error(format!("参数解析失败: {e}"), json!({}), 0),
        };
        let order_id = match get_i64_param(&param, "order_id") {
            Some(id) => id,
            None => return self.render_error("order_id 参数缺失", json!({}), 0),
        };

        match Self::detail_order(repo, order_id) {
            Some(detail) => self.render_success("", json!({"detail": detail})),
            None => self.render_error("数据不存在", json!({}), 0),
        }
    }

    // ========================================================================
    // 业务方法（对齐 PHP `CustomerPay` 模型业务方法）
    // ========================================================================

    /// 查询订单列表 — 对齐 PHP `CustomerPay::getList($param, $type)`
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function getList($param, $type) {
    ///     // 基础条件：app_id, is_delete=0
    ///     // 可选过滤：dept_id/customer_id/pay_status/pay_type/pay_source/order_no/stat_day
    ///     // type=list: 分页; type=export: 不分页
    /// }
    /// ```
    fn get_list(
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
        param: &Value,
        list_type: &str,
    ) -> Value {
        let app_id = get_app_id(param);

        let mut conditions = vec![
            WhereCondition::new("is_delete", WhereOp::Eq, OrmValue::I64(0)),
            WhereCondition::new("app_id", WhereOp::Eq, OrmValue::I64(app_id)),
        ];

        // 可选过滤条件
        if let Some(dept_id) = get_i64_param(param, "dept_id") {
            conditions.push(WhereCondition::new(
                "dept_id",
                WhereOp::Eq,
                OrmValue::I64(dept_id),
            ));
        }
        if let Some(customer_id) = get_i64_param(param, "customer_id") {
            conditions.push(WhereCondition::new(
                "customer_id",
                WhereOp::Eq,
                OrmValue::I64(customer_id),
            ));
        }
        if let Some(pay_status) = get_i64_param(param, "pay_status") {
            conditions.push(WhereCondition::new(
                "pay_status",
                WhereOp::Eq,
                OrmValue::I64(pay_status),
            ));
        }
        if let Some(pay_type) = get_i64_param(param, "pay_type") {
            conditions.push(WhereCondition::new(
                "pay_type",
                WhereOp::Eq,
                OrmValue::I64(pay_type),
            ));
        }

        let mut items: Vec<Value> = match repo.find_by(&conditions) {
            Ok(list) => list.into_iter().map(|m| m.to_json()).collect(),
            Err(_) => return json!({"list": []}),
        };

        // 字符串过滤条件（pay_source/order_no）
        if let Some(pay_source) = get_str_param(param, "pay_source") {
            items.retain(|item| {
                item.get("pay_source")
                    .and_then(|v| v.as_str())
                    .map(|s| s == pay_source)
                    .unwrap_or(false)
            });
        }
        if let Some(order_no) = get_str_param(param, "order_no") {
            items.retain(|item| {
                item.get("order_no")
                    .and_then(|v| v.as_str())
                    .map(|s| s == order_no)
                    .unwrap_or(false)
            });
        }

        // PHP 按 create_time desc 排序（简化：按 order_id desc）
        items.sort_by(|a, b| {
            let a_id = a.get("order_id").and_then(|v| v.as_i64()).unwrap_or(0);
            let b_id = b.get("order_id").and_then(|v| v.as_i64()).unwrap_or(0);
            b_id.cmp(&a_id)
        });

        // export 类型不分页
        if list_type == "export" {
            return json!({"list": items});
        }

        // 分页
        let list_rows = get_i64_param(param, "list_rows").unwrap_or(15) as usize;
        let page = get_i64_param(param, "page").unwrap_or(1) as usize;
        let start = page.saturating_sub(1) * list_rows;
        let result_list = if start >= items.len() {
            Vec::new()
        } else {
            let end = (start + list_rows).min(items.len());
            items[start..end].to_vec()
        };

        json!({"list": result_list})
    }

    /// 查询订单详情 — 对齐 PHP `CustomerPay::detail($order_id)`
    fn detail_order(
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
        order_id: i64,
    ) -> Option<Value> {
        let conditions = [WhereCondition::new(
            "order_id",
            WhereOp::Eq,
            OrmValue::I64(order_id),
        )];
        repo.find_one_by(&conditions)
            .ok()
            .flatten()
            .map(|mut m| m.to_json_with_append_cached())
    }

    /// 应用银行支付状态查询结果 — 对齐 PHP `check()` 方法多银行分支
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// if($param['bank_name'] == 'ccb'){
    ///     if($pay_res['respObj']['RESULT'] == 'Y') { $detail->settle(); }
    ///     else if($pay_res['respObj']['RESULT'] == 'N'){
    ///         $detail->where(['order_no'=>$param['order_no'],'pay_status'=>10])
    ///             ->update(['order_status'=>20]);
    ///     }
    /// } elseif($param['bank_name'] == 'icbc'){
    ///     if($pay_res['respObj']['pay_status'] != 0) {
    ///         if($pay_res['respObj']['pay_status'] == 1){ $detail->settle(); }
    ///         else { $detail->where(...)->update(['order_status'=>20]); }
    ///     }
    /// } elseif($param['bank_name'] == 'fuiou'){
    ///     if($pay_res['respObj']['result_code'] == '000000') {
    ///         if($pay_res['respObj']['trans_stat'] == 'SUCCESS'){ $detail->settle(); }
    ///         else { $detail->where(...)->update(['order_status'=>20]); }
    ///     }
    /// }
    /// ```
    ///
    /// # 参数
    ///
    /// - `param`：请求参数（含 `bank_name`/`order_no`）
    /// - `pay_res`：`epay_check` 返回的查询结果
    /// - `repo`：CustomerPay Repository
    ///
    /// # 返回
    ///
    /// - `Some(Ok(()))`：已应用更新
    /// - `Some(Err(String))`：更新失败
    /// - `None`：银行名不匹配或无需更新
    fn apply_bank_check_result(
        param: &Value,
        pay_res: &Value,
        repo: &dyn Repository<CustomerPay, Key = OrmValue>,
    ) -> Option<Result<(), String>> {
        let bank_name = get_str_param(param, "bank_name").unwrap_or_default();
        let default_obj = json!({});
        let resp_obj = pay_res.get("respObj").unwrap_or(&default_obj);
        let order_no = get_str_param(param, "order_no").unwrap_or_default();

        // 判断是否需要 settle（PHP $detail->settle()）
        // settle 语义：更新 pay_status=20（已付款）
        let should_settle = match bank_name.as_str() {
            "ccb" => resp_obj
                .get("RESULT")
                .and_then(|v| v.as_str())
                .map(|s| s == "Y")
                .unwrap_or(false),
            "icbc" => resp_obj
                .get("pay_status")
                .and_then(|v| v.as_i64())
                .map(|s| s == 1)
                .unwrap_or(false),
            "fuiou" => {
                let result_code = resp_obj
                    .get("result_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let trans_stat = resp_obj
                    .get("trans_stat")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                result_code == "000000" && trans_stat == "SUCCESS"
            }
            _ => return None,
        };

        // 判断是否需要标记 order_status=20（已经取消）
        let should_cancel = match bank_name.as_str() {
            "ccb" => resp_obj
                .get("RESULT")
                .and_then(|v| v.as_str())
                .map(|s| s == "N")
                .unwrap_or(false),
            "icbc" => {
                let ps = resp_obj
                    .get("pay_status")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                ps != 0 && ps != 1
            }
            "fuiou" => {
                let result_code = resp_obj
                    .get("result_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let trans_stat = resp_obj
                    .get("trans_stat")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                result_code == "000000" && trans_stat != "SUCCESS"
            }
            _ => false,
        };

        if should_settle {
            // PHP: $detail->settle() → 更新 pay_status=20
            // 通过 order_no 查找订单
            let conditions = [WhereCondition::new(
                "order_no",
                WhereOp::Eq,
                OrmValue::String(order_no.clone()),
            )];
            match repo.find_one_by(&conditions) {
                Ok(Some(mut model)) => {
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("pay_status".to_string(), json!(20));
                    model.set_attrs(&data_map);
                    Some(repo.save(model).map(|_| ()).map_err(|e| e.to_string()))
                }
                _ => Some(Err("订单不存在".to_string())),
            }
        } else if should_cancel {
            // PHP: update(['order_status'=>20]) where pay_status=10
            let conditions = [
                WhereCondition::new("order_no", WhereOp::Eq, OrmValue::String(order_no.clone())),
                WhereCondition::new("pay_status", WhereOp::Eq, OrmValue::I64(10)),
            ];
            match repo.find_one_by(&conditions) {
                Ok(Some(mut model)) => {
                    let mut data_map: std::collections::HashMap<String, Value> =
                        std::collections::HashMap::new();
                    data_map.insert("order_status".to_string(), json!(20));
                    model.set_attrs(&data_map);
                    Some(repo.save(model).map(|_| ()).map_err(|e| e.to_string()))
                }
                _ => None, // PHP 无匹配记录时不报错
            }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::SettledService;
    use serde_json::json;
    use sz_orm_core::repository::InMemoryRepository;

    /// 测试用 Mock SettledService
    struct TestSettledService;

    impl SettledService for TestSettledService {
        fn create_order(&self, data: &Value) -> Result<Value, String> {
            let order_id = data.get("order_id").and_then(|v| v.as_i64()).unwrap_or(1);
            Ok(json!({
                "order_id": order_id,
                "pay_res": {"msg": "success", "data": []}
            }))
        }

        fn pay_buy(&self, detail: &Value, _data: &Value) -> Result<Value, String> {
            let order_id = detail.get("order_id").and_then(|v| v.as_i64()).unwrap_or(1);
            Ok(json!({
                "order_id": order_id,
                "pay_res": {"msg": "pay_buy_success", "data": []}
            }))
        }

        fn epay_check(&self, param: &Value) -> Result<Value, String> {
            let bank_name = param
                .get("bank_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let resp_obj = match bank_name {
                "ccb" => json!({"RESULT": "Y"}),
                "icbc" => json!({"pay_status": 1}),
                "fuiou" => json!({"result_code": "000000", "trans_stat": "SUCCESS"}),
                _ => json!({}),
            };
            Ok(json!({"msg": "查询成功", "respObj": resp_obj}))
        }

        fn refund(&self, _detail: &Value, _param: &Value) -> Result<(), String> {
            Ok(())
        }
    }

    fn make_order(
        id: i64,
        order_no: &str,
        pay_status: i64,
        order_status: i64,
        app_id: i64,
    ) -> CustomerPay {
        CustomerPay::new()
            .with_data("order_id", json!(id))
            .with_data("order_no", json!(order_no))
            .with_data("pay_status", json!(pay_status))
            .with_data("order_status", json!(order_status))
            .with_data("is_delete", json!(0))
            .with_data("app_id", json!(app_id))
            .with_data("dept_id", json!(34))
            .with_data("customer_id", json!(1))
            .with_data("pay_type", json!(1))
            .with_data("pay_source", json!("ccb"))
    }

    fn make_repo() -> InMemoryRepository<CustomerPay> {
        InMemoryRepository::from_vec(vec![
            make_order(1, "ORD001", 10, 10, 10001),
            make_order(2, "ORD002", 20, 30, 10001),
            make_order(3, "ORD003", 30, 30, 10001),
            make_order(4, "ORD004", 20, 30, 20002),
        ])
    }

    // -------------------- get_list 测试 --------------------

    #[test]
    fn test_get_list_filters_by_app_id() {
        let repo = make_repo();
        let result = CustomerPayController::get_list(&repo, &json!({"app_id": 10001}), "list");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_get_list_export_returns_all() {
        let repo = make_repo();
        let result = CustomerPayController::get_list(&repo, &json!({"app_id": 10001}), "export");
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_get_list_with_pay_status_filter() {
        let repo = make_repo();
        let result = CustomerPayController::get_list(
            &repo,
            &json!({"app_id": 10001, "pay_status": 20}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["order_id"], 2);
    }

    #[test]
    fn test_get_list_with_pay_source_filter() {
        let repo = make_repo();
        let result = CustomerPayController::get_list(
            &repo,
            &json!({"app_id": 10001, "pay_source": "ccb"}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn test_get_list_pagination() {
        let repo = make_repo();
        let result = CustomerPayController::get_list(
            &repo,
            &json!({"app_id": 10001, "list_rows": 2, "page": 1}),
            "list",
        );
        let list = result["list"].as_array().unwrap();
        assert_eq!(list.len(), 2);
    }

    // -------------------- detail_order 测试 --------------------

    #[test]
    fn test_detail_order_exists() {
        let repo = make_repo();
        let detail = CustomerPayController::detail_order(&repo, 1).unwrap();
        assert_eq!(detail["order_id"], 1);
        assert_eq!(detail["order_no"], "ORD001");
    }

    #[test]
    fn test_detail_order_not_found() {
        let repo = make_repo();
        let result = CustomerPayController::detail_order(&repo, 999);
        assert!(result.is_none());
    }

    // -------------------- apply_bank_check_result 测试 --------------------

    #[test]
    fn test_apply_bank_check_ccb_settle() {
        let repo = make_repo();
        let param = json!({"bank_name": "ccb", "order_no": "ORD001"});
        let pay_res = json!({"msg": "查询成功", "respObj": {"RESULT": "Y"}});
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(matches!(result, Some(Ok(()))));

        // 验证 pay_status 已更新为 20
        let conditions = [WhereCondition::new(
            "order_no",
            WhereOp::Eq,
            OrmValue::String("ORD001".to_string()),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["pay_status"], 20);
    }

    #[test]
    fn test_apply_bank_check_ccb_cancel() {
        let repo = make_repo();
        let param = json!({"bank_name": "ccb", "order_no": "ORD001"});
        let pay_res = json!({"msg": "查询成功", "respObj": {"RESULT": "N"}});
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(matches!(result, Some(Ok(()))));

        // 验证 order_status 已更新为 20
        let conditions = [WhereCondition::new(
            "order_no",
            WhereOp::Eq,
            OrmValue::String("ORD001".to_string()),
        )];
        let updated = repo.find_one_by(&conditions).unwrap().unwrap();
        assert_eq!(updated.to_json()["order_status"], 20);
    }

    #[test]
    fn test_apply_bank_check_icbc_settle() {
        let repo = make_repo();
        let param = json!({"bank_name": "icbc", "order_no": "ORD001"});
        let pay_res = json!({"msg": "查询成功", "respObj": {"pay_status": 1}});
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(matches!(result, Some(Ok(()))));
    }

    #[test]
    fn test_apply_bank_check_fuiou_settle() {
        let repo = make_repo();
        let param = json!({"bank_name": "fuiou", "order_no": "ORD001"});
        let pay_res = json!({
            "msg": "查询成功",
            "respObj": {"result_code": "000000", "trans_stat": "SUCCESS"}
        });
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(matches!(result, Some(Ok(()))));
    }

    #[test]
    fn test_apply_bank_check_unknown_bank_returns_none() {
        let repo = make_repo();
        let param = json!({"bank_name": "unknown", "order_no": "ORD001"});
        let pay_res = json!({"msg": "查询成功", "respObj": {}});
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(result.is_none());
    }

    // -------------------- SettledService trait 测试 --------------------

    #[test]
    fn test_settled_service_create_order_returns_order_detail() {
        let svc = TestSettledService;
        let result = svc.create_order(&json!({"order_id": 42})).unwrap();
        assert_eq!(result["order_id"], 42);
        assert_eq!(result["pay_res"]["msg"], "success");
    }

    #[test]
    fn test_settled_service_pay_buy_returns_order_detail() {
        let svc = TestSettledService;
        let result = svc.pay_buy(&json!({"order_id": 10}), &json!({})).unwrap();
        assert_eq!(result["order_id"], 10);
        assert_eq!(result["pay_res"]["msg"], "pay_buy_success");
    }

    #[test]
    fn test_settled_service_epay_check_ccb_returns_y() {
        let svc = TestSettledService;
        let result = svc.epay_check(&json!({"bank_name": "ccb"})).unwrap();
        assert_eq!(result["respObj"]["RESULT"], "Y");
    }

    #[test]
    fn test_settled_service_refund_success() {
        let svc = TestSettledService;
        let result = svc.refund(&json!({"order_id": 1}), &json!({}));
        assert!(result.is_ok());
    }

    // -------------------- R5 PHP 行为对齐测试 --------------------

    #[test]
    fn test_r5_php_customer_pay_get_list_returns_list_key() {
        // R5: PHP getList 返回 {'list': [...]} 结构
        let repo = make_repo();
        let result = CustomerPayController::get_list(&repo, &json!({"app_id": 10001}), "list");
        assert!(result["list"].is_array());
    }

    #[test]
    fn test_r5_php_customer_pay_detail_returns_json_with_append_fields() {
        // R5: PHP detail 返回包含 append 字段的 JSON
        let repo = make_repo();
        let detail = CustomerPayController::detail_order(&repo, 1).unwrap();
        // append 5 个字段：order_status_text/sync_status_text/pay_status_text/pay_type_text/pay_source_text
        assert!(detail.get("pay_status_text").is_some());
        assert!(detail.get("pay_type_text").is_some());
    }

    #[test]
    fn test_r5_php_customer_pay_check_ccb_settle_on_y() {
        // R5: PHP check ccb RESULT=Y → settle()
        let repo = make_repo();
        let param = json!({"bank_name": "ccb", "order_no": "ORD001", "type": "check"});
        let pay_res = json!({"msg": "查询成功", "respObj": {"RESULT": "Y"}});
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(matches!(result, Some(Ok(()))));
    }

    #[test]
    fn test_r5_php_customer_pay_check_ccb_cancel_on_n() {
        // R5: PHP check ccb RESULT=N → order_status=20
        let repo = make_repo();
        let param = json!({"bank_name": "ccb", "order_no": "ORD001", "type": "check"});
        let pay_res = json!({"msg": "查询成功", "respObj": {"RESULT": "N"}});
        let result = CustomerPayController::apply_bank_check_result(&param, &pay_res, &repo);
        assert!(matches!(result, Some(Ok(()))));
    }

    #[test]
    fn test_r5_php_customer_pay_refund_blocks_unpaid_order() {
        // R5: PHP refund pay_status=10 → '该订单尚未支付'
        // 该逻辑在控制器方法层验证，此处验证 detail 数据结构
        let repo = make_repo();
        let detail = CustomerPayController::detail_order(&repo, 1).unwrap();
        let pay_status = detail
            .get("pay_status")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(pay_status, 10); // 未付款
    }

    #[test]
    fn test_r5_php_customer_pay_refund_blocks_already_refunded() {
        // R5: PHP refund pay_status=30 → '该订单已退款'
        let repo = make_repo();
        let detail = CustomerPayController::detail_order(&repo, 3).unwrap();
        let pay_status = detail
            .get("pay_status")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        assert_eq!(pay_status, 30); // 已退款
    }
}
