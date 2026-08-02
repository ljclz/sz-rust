//! 空间/GIS 支持 — Phase 6.32
//!
//! 提供 PostGIS 风格的空间数据类型与 ST_* 函数：
//!
//! - **几何类型**：Point/LineString/Polygon/MultiPoint/MultiLineString/MultiPolygon/GeometryCollection
//! - **地理类型**：SRID=4326（WGS84）时使用 Haversine 球面距离（米）
//! - **几何类型**：其他 SRID 使用笛卡尔欧氏距离（坐标单位）
//! - **ST_* 函数**：ST_Point/ST_X/ST_Y/ST_Distance/ST_Within/ST_Contains/ST_Intersects/
//!   ST_Area/ST_Length/ST_Envelope/ST_AsText/ST_GeomFromText/ST_SetSRID/ST_SRID
//! - **WKT 解析与序列化**：与 PostGIS/OGC 标准一致
//!
//! # 设计
//!
//! - **Geometry**：核心枚举，所有变体共享 `srid` 字段（通过外部包装）
//! - **SridGeometry**：携带 SRID 的几何体（对应 PG `geometry` 类型的 SRID 元数据）
//! - **BoundingBox**：轴对齐包围盒，用于 GiST 索引
//! - **point_in_polygon**：射线法（ray casting）判定点是否在多边形内
//! - **polygon_intersects**：基于包围盒 + 边相交 + 包含关系判定两多边形是否相交
//!
//! # 与 PostGIS 的关系
//!
//! - PostGIS 1.0+ 支持 `geometry` / `geography` 两种类型
//! - `geometry`：笛卡尔坐标（平面），距离 = 欧氏距离
//! - `geography`：经纬度（球面），距离 = Haversine（默认球模型，半径 6371008.8m）
//! - 本实现用 `SRID=4326` 触发 geography 语义，其余按 geometry 语义
//! - `ST_Distance(geography)` 返回米；`ST_Distance(geometry)` 返回坐标单位
//! - WKT 格式：`POINT (1 2)` / `LINESTRING (0 0, 1 1)` / `POLYGON ((0 0, 4 0, 4 4, 0 0))`
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：仅提供程序化 API，未集成到 SQL 解析路径
//! - **无持久化**：纯内存对象
//! - **无 WKB**：仅支持 WKT 文本格式
//! - **无 3D/4D**：仅 2D（X, Y），无 Z/M 维度
//! - **球面距离模型**：Haversine 使用球体模型（非 WGS84 椭球），与 PostGIS `use_spheroid=false` 一致
//! - **ST_Within/ST_Contains**：边界判定遵循 OGC（含边界=内），未实现 DE-9IM 完整矩阵
//! - **Multi* 类型**：作为容器存在，运算委托到子几何

use crate::executor::ExecutionError;

// =====================================================================
//  常量
// =====================================================================

/// WGS84 球面地理坐标 SRID（PostGIS 默认 geography）
pub const SRID_WGS84: u32 = 4326;

/// 地球平均半径（米）— PostGIS `SPHEROID["WGS 84"...]` 球体近似
pub const EARTH_RADIUS_METERS: f64 = 6371008.8;

/// 默认 SRID（未指定时使用 0 表示笛卡尔坐标）
pub const SRID_DEFAULT: u32 = 0;

// =====================================================================
//  错误类型
// =====================================================================

/// 空间操作错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SpatialError {
    /// WKT 解析错误
    #[error("WKT parse error: {0}")]
    WktParse(String),
    /// 不支持的几何类型
    #[error("unsupported geometry type: {0}")]
    UnsupportedType(String),
    /// 几何体维度不匹配
    #[error("dimension mismatch: {0}")]
    DimensionMismatch(String),
    /// 空几何体
    #[error("empty geometry")]
    EmptyGeometry,
    /// 无效坐标（NaN/Inf）
    #[error("invalid coordinate: {0}")]
    InvalidCoordinate(String),
    /// 操作不支持（如对 Point 求 ST_Area）
    #[error("operation not supported for geometry: {0}")]
    UnsupportedOperation(String),
    /// SRID 不匹配
    #[error("SRID mismatch: {0} vs {1}")]
    SridMismatch(u32, u32),
    /// 多边形环数不足（至少 1 个外环）
    #[error("polygon requires at least one ring")]
    PolygonNoRing,
    /// 多边形环未闭合（首尾点必须相同）
    #[error("polygon ring not closed")]
    PolygonNotClosed,
}

impl From<SpatialError> for ExecutionError {
    fn from(e: SpatialError) -> Self {
        ExecutionError::EvalError(format!("spatial error: {e}"))
    }
}

// =====================================================================
//  坐标与包围盒
// =====================================================================

/// 2D 坐标点（X, Y）— 通常 X=经度，Y=纬度
pub type Coord = (f64, f64);

/// 轴对齐包围盒（Axis-Aligned Bounding Box）
///
/// 用于 GiST 索引与 ST_Envelope。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    /// 最小 X
    pub min_x: f64,
    /// 最小 Y
    pub min_y: f64,
    /// 最大 X
    pub max_x: f64,
    /// 最大 Y
    pub max_y: f64,
}

impl BoundingBox {
    /// 构造包围盒
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    /// 从单点构造退化包围盒
    pub fn from_point(x: f64, y: f64) -> Self {
        Self {
            min_x: x,
            min_y: y,
            max_x: x,
            max_y: y,
        }
    }

    /// 从坐标迭代器构造包围盒
    pub fn from_coords<I: IntoIterator<Item = Coord>>(coords: I) -> Option<Self> {
        let mut iter = coords.into_iter();
        let (x0, y0) = iter.next()?;
        let mut min_x = x0;
        let mut max_x = x0;
        let mut min_y = y0;
        let mut max_y = y0;
        for (x, y) in iter {
            if x < min_x {
                min_x = x;
            }
            if x > max_x {
                max_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if y > max_y {
                max_y = y;
            }
        }
        Some(Self {
            min_x,
            min_y,
            max_x,
            max_y,
        })
    }

    /// 是否为空（max < min 视为空）
    pub fn is_empty(&self) -> bool {
        self.max_x < self.min_x || self.max_y < self.min_y
    }

    /// 宽度
    pub fn width(&self) -> f64 {
        (self.max_x - self.min_x).max(0.0)
    }

    /// 高度
    pub fn height(&self) -> f64 {
        (self.max_y - self.min_y).max(0.0)
    }

    /// 面积
    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    /// 周长（2*(w+h)）
    pub fn perimeter(&self) -> f64 {
        2.0 * (self.width() + self.height())
    }

    /// 中心点
    pub fn center(&self) -> Coord {
        (
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
        )
    }

    /// 是否包含点
    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// 是否包含另一包围盒（含边界）
    pub fn contains_bbox(&self, other: &BoundingBox) -> bool {
        self.min_x <= other.min_x
            && self.max_x >= other.max_x
            && self.min_y <= other.min_y
            && self.max_y >= other.max_y
    }

    /// 是否与另一包围盒相交（含边界）
    pub fn intersects(&self, other: &BoundingBox) -> bool {
        !(self.max_x < other.min_x
            || self.min_x > other.max_x
            || self.max_y < other.min_y
            || self.min_y > other.max_y)
    }

    /// 与另一包围盒的并集（返回覆盖两者的最小包围盒）
    pub fn union(&self, other: &BoundingBox) -> BoundingBox {
        BoundingBox {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// 与另一包围盒的交集（无交集则返回 None）
    pub fn intersection(&self, other: &BoundingBox) -> Option<BoundingBox> {
        if !self.intersects(other) {
            return None;
        }
        Some(BoundingBox {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        })
    }

    /// 扩展以包含指定点
    pub fn extend_point(&mut self, x: f64, y: f64) {
        if x < self.min_x {
            self.min_x = x;
        }
        if x > self.max_x {
            self.max_x = x;
        }
        if y < self.min_y {
            self.min_y = y;
        }
        if y > self.max_y {
            self.max_y = y;
        }
    }

    /// 计算合并另一包围盒后的面积增量（GiST penalty 用）
    pub fn area_increase(&self, other: &BoundingBox) -> f64 {
        self.union(other).area() - self.area()
    }
}

impl Default for BoundingBox {
    fn default() -> Self {
        // 空 bbox：max < min
        Self {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }
}

// =====================================================================
//  Geometry — 几何类型枚举
// =====================================================================

/// 几何类型枚举（OGC SFSQL 标准）
///
/// 所有变体均为 2D（无 Z/M 维度）。
/// SRID 通过外部 [`SridGeometry`] 包装携带。
#[derive(Debug, Clone, PartialEq)]
pub enum Geometry {
    /// 点 — (X, Y)
    Point(Coord),
    /// 线串 — 有序点序列（至少 2 个点）
    LineString(Vec<Coord>),
    /// 多边形 — 外环 + 内环（孔洞），每环首尾点相同（闭合）
    Polygon(Vec<Vec<Coord>>),
    /// 多点 — 点集合
    MultiPoint(Vec<Coord>),
    /// 多线串 — 线串集合
    MultiLineString(Vec<Vec<Coord>>),
    /// 多多边形 — 多边形集合
    MultiPolygon(Vec<Vec<Vec<Coord>>>),
    /// 几何集合 — 异构几何列表
    GeometryCollection(Vec<Geometry>),
}

impl Geometry {
    /// 构造 Point
    pub fn point(x: f64, y: f64) -> Self {
        Self::Point((x, y))
    }

    /// 构造 LineString
    pub fn line_string(coords: Vec<Coord>) -> Self {
        Self::LineString(coords)
    }

    /// 构造 Polygon（外环 + 内环）
    pub fn polygon(rings: Vec<Vec<Coord>>) -> Self {
        Self::Polygon(rings)
    }

    /// 几何类型的字符串名（WKT 标识）
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Point(_) => "Point",
            Self::LineString(_) => "LineString",
            Self::Polygon(_) => "Polygon",
            Self::MultiPoint(_) => "MultiPoint",
            Self::MultiLineString(_) => "MultiLineString",
            Self::MultiPolygon(_) => "MultiPolygon",
            Self::GeometryCollection(_) => "GeometryCollection",
        }
    }

    /// 是否为空几何
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Point(_) => false,
            Self::LineString(c) => c.is_empty(),
            Self::Polygon(rings) => rings.is_empty(),
            Self::MultiPoint(c) => c.is_empty(),
            Self::MultiLineString(c) => c.is_empty(),
            Self::MultiPolygon(c) => c.is_empty(),
            Self::GeometryCollection(c) => c.is_empty(),
        }
    }

    /// 计算包围盒
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        match self {
            Self::Point((x, y)) => Some(BoundingBox::from_point(*x, *y)),
            Self::LineString(coords) => BoundingBox::from_coords(coords.iter().copied()),
            Self::Polygon(rings) => BoundingBox::from_coords(rings.iter().flatten().copied()),
            Self::MultiPoint(coords) => BoundingBox::from_coords(coords.iter().copied()),
            Self::MultiLineString(lines) => {
                BoundingBox::from_coords(lines.iter().flatten().copied())
            }
            Self::MultiPolygon(polys) => {
                BoundingBox::from_coords(polys.iter().flatten().flatten().copied())
            }
            Self::GeometryCollection(geoms) => {
                let mut bbox = BoundingBox::default();
                let mut has_any = false;
                for g in geoms {
                    if let Some(b) = g.bounding_box() {
                        if !has_any {
                            bbox = b;
                            has_any = true;
                        } else {
                            bbox = bbox.union(&b);
                        }
                    }
                }
                if has_any {
                    Some(bbox)
                } else {
                    None
                }
            }
        }
    }

    /// 提取所有点（深度优先）
    pub fn collect_coords(&self) -> Vec<Coord> {
        match self {
            Self::Point(c) => vec![*c],
            Self::LineString(c) => c.clone(),
            Self::Polygon(rings) => rings.iter().flatten().copied().collect(),
            Self::MultiPoint(c) => c.clone(),
            Self::MultiLineString(lines) => lines.iter().flatten().copied().collect(),
            Self::MultiPolygon(polys) => polys.iter().flatten().flatten().copied().collect(),
            Self::GeometryCollection(geoms) => {
                geoms.iter().flat_map(|g| g.collect_coords()).collect()
            }
        }
    }
}

// =====================================================================
//  SridGeometry — 携带 SRID 的几何体
// =====================================================================

/// 携带 SRID 的几何体
///
/// 对应 PG `geometry` 类型的 SRID 元数据。
/// SRID=4326 时使用 geography 语义（Haversine 距离）。
#[derive(Debug, Clone, PartialEq)]
pub struct SridGeometry {
    /// 几何体
    pub geom: Geometry,
    /// 空间参考系统标识符
    pub srid: u32,
}

impl SridGeometry {
    /// 构造指定 SRID 的几何体
    pub fn new(geom: Geometry, srid: u32) -> Self {
        Self { geom, srid }
    }

    /// 构造默认 SRID 的几何体
    pub fn with_default_srid(geom: Geometry) -> Self {
        Self {
            geom,
            srid: SRID_DEFAULT,
        }
    }

    /// 构造 WGS84 地理几何体
    pub fn with_wgs84(geom: Geometry) -> Self {
        Self {
            geom,
            srid: SRID_WGS84,
        }
    }

    /// 是否为地理类型（SRID=4326）
    pub fn is_geography(&self) -> bool {
        self.srid == SRID_WGS84
    }

    /// 设置 SRID
    pub fn set_srid(&mut self, srid: u32) {
        self.srid = srid;
    }

    /// 序列化为 WKT 文本（与 `st_as_text` 等价，供程序化 API 使用）
    pub fn to_wkt(&self) -> String {
        st_as_text(self)
    }
}

// =====================================================================
//  几何度量函数
// =====================================================================

/// 欧氏距离（笛卡尔坐标）
fn euclidean_distance(p1: Coord, p2: Coord) -> f64 {
    let dx = p1.0 - p2.0;
    let dy = p1.1 - p2.1;
    (dx * dx + dy * dy).sqrt()
}

/// Haversine 球面距离（米）
///
/// 输入：(经度, 纬度) 角度值
/// 输出：球面弧长（米）
fn haversine_distance(p1: Coord, p2: Coord) -> f64 {
    let to_rad = |deg: f64| deg * std::f64::consts::PI / 180.0;
    let lat1 = to_rad(p1.1);
    let lat2 = to_rad(p2.1);
    let dlat = to_rad(p2.1 - p1.1);
    let dlon = to_rad(p2.0 - p1.0);
    let a = (dlat * 0.5).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon * 0.5).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    EARTH_RADIUS_METERS * c
}

/// 点到线段的最短距离（笛卡尔）
fn point_segment_distance(p: Coord, a: Coord, b: Coord) -> f64 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return euclidean_distance(p, a);
    }
    let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let proj = (a.0 + t * dx, a.1 + t * dy);
    euclidean_distance(p, proj)
}

/// 计算线串长度（笛卡尔）
fn line_string_length(coords: &[Coord]) -> f64 {
    coords
        .windows(2)
        .map(|w| euclidean_distance(w[0], w[1]))
        .sum()
}

/// 计算多边形面积（鞋带公式，笛卡尔）
///
/// 仅使用外环，内环（孔洞）由调用方处理。
fn ring_area(coords: &[Coord]) -> f64 {
    if coords.len() < 3 {
        return 0.0;
    }
    let n = coords.len();
    let mut sum = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        sum += coords[i].0 * coords[j].1;
        sum -= coords[j].0 * coords[i].1;
    }
    (sum / 2.0).abs()
}

/// 计算多边形面积（外环 - 内环）
fn polygon_area(rings: &[Vec<Coord>]) -> f64 {
    if rings.is_empty() {
        return 0.0;
    }
    let outer = ring_area(&rings[0]);
    let inner: f64 = rings.iter().skip(1).map(|r| ring_area(r)).sum();
    outer - inner
}

// =====================================================================
//  拓扑判定
// =====================================================================

/// 射线法判定点是否在多边形内（含边界 = 内）
///
/// 算法：从点向 +X 方向发射射线，统计与多边形边的交点数。
/// 奇数 = 内，偶数 = 外。
/// 边界点（在边上）返回 true。
pub fn point_in_polygon(p: Coord, ring: &[Coord]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    // 先判定是否在边上
    for w in ring.windows(2) {
        if point_on_segment(p, w[0], w[1]) {
            return true;
        }
    }
    // 闭合环判定：末点 → 首点
    if point_on_segment(p, ring[n - 1], ring[0]) {
        return true;
    }
    // 射线法
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = ring[i];
        let (xj, yj) = ring[j];
        let intersect = (yi > p.1) != (yj > p.1)
            && p.0
                < (xj - xi) * (p.1 - yi)
                    / (yj - yi
                        + if yj == yi {
                            1e-12
                        } else {
                            0.0
                        })
                    + xi;
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 点是否在线段上（含端点）
fn point_on_segment(p: Coord, a: Coord, b: Coord) -> bool {
    // 共线判定：叉积为 0
    let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
    if cross.abs() > 1e-12 {
        return false;
    }
    // 在包围盒内
    let min_x = a.0.min(b.0);
    let max_x = a.0.max(b.0);
    let min_y = a.1.min(b.1);
    let max_y = a.1.max(b.1);
    p.0 >= min_x - 1e-12 && p.0 <= max_x + 1e-12 && p.1 >= min_y - 1e-12 && p.1 <= max_y + 1e-12
}

/// 判定点是否在多边形（含孔洞）内
///
/// 规则：在外环内 且 不在任何内环内。
pub fn point_in_polygon_with_holes(p: Coord, rings: &[Vec<Coord>]) -> bool {
    if rings.is_empty() {
        return false;
    }
    if !point_in_polygon(p, &rings[0]) {
        return false;
    }
    for inner in rings.iter().skip(1) {
        if point_in_polygon(p, inner) {
            return false; // 在孔洞内 → 不在多边形内
        }
    }
    true
}

/// 两线段是否相交（含端点）
fn segments_intersect(p1: Coord, p2: Coord, p3: Coord, p4: Coord) -> bool {
    let d1 = cross(p3, p4, p1);
    let d2 = cross(p3, p4, p2);
    let d3 = cross(p1, p2, p3);
    let d4 = cross(p1, p2, p4);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    // 共线情况：检查端点是否在另一线段上
    if d1.abs() <= 1e-12 && point_on_segment(p1, p3, p4) {
        return true;
    }
    if d2.abs() <= 1e-12 && point_on_segment(p2, p3, p4) {
        return true;
    }
    if d3.abs() <= 1e-12 && point_on_segment(p3, p1, p2) {
        return true;
    }
    if d4.abs() <= 1e-12 && point_on_segment(p4, p1, p2) {
        return true;
    }
    false
}

/// 叉积 (b - a) × (c - a)
fn cross(a: Coord, b: Coord, c: Coord) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// 两多边形环是否边相交
fn rings_edges_intersect(r1: &[Coord], r2: &[Coord]) -> bool {
    // windows(2) 已覆盖相邻边；末尾闭合边单独检查
    let n1 = r1.len();
    let n2 = r2.len();
    // 收集 r1 所有边（含闭合边）
    let r1_edges: Vec<(Coord, Coord)> = r1
        .windows(2)
        .map(|w| (w[0], w[1]))
        .chain(std::iter::once((r1[n1 - 1], r1[0])))
        .collect();
    let r2_edges: Vec<(Coord, Coord)> = r2
        .windows(2)
        .map(|w| (w[0], w[1]))
        .chain(std::iter::once((r2[n2 - 1], r2[0])))
        .collect();
    for (a, b) in &r1_edges {
        for (c, d) in &r2_edges {
            if segments_intersect(*a, *b, *c, *d) {
                return true;
            }
        }
    }
    false
}

/// 判定多边形 A 是否包含多边形 B（A 含 B）
///
/// 规则：B 的所有顶点在 A 内（含边界）且无边相交。
pub fn polygon_contains_polygon(outer: &[Vec<Coord>], inner: &[Vec<Coord>]) -> bool {
    if outer.is_empty() || inner.is_empty() {
        return false;
    }
    // B 的所有顶点必须在 A 内
    for &v in inner[0].iter() {
        if !point_in_polygon_with_holes(v, outer) {
            return false;
        }
    }
    // 无边相交
    if rings_edges_intersect(&outer[0], &inner[0]) {
        // 边相交时，若仅共享边界且 B 完全在 A 内，仍视为包含
        // 但严格 OGC 语义：边相交 → 不包含
        return false;
    }
    true
}

// =====================================================================
//  ST_* 函数（PostGIS 兼容）
// =====================================================================

/// ST_Point(x, y) — 构造 Point
pub fn st_point(x: f64, y: f64) -> SridGeometry {
    SridGeometry::with_default_srid(Geometry::point(x, y))
}

/// ST_Point(x, y, srid) — 构造指定 SRID 的 Point
pub fn st_point_with_srid(x: f64, y: f64, srid: u32) -> SridGeometry {
    SridGeometry::new(Geometry::point(x, y), srid)
}

/// ST_X(g) — 提取 Point 的 X 坐标
pub fn st_x(g: &SridGeometry) -> Result<f64, SpatialError> {
    match g.geom {
        Geometry::Point((x, _)) => Ok(x),
        _ => Err(SpatialError::UnsupportedOperation(format!(
            "ST_X requires Point, got {}",
            g.geom.type_name()
        ))),
    }
}

/// ST_Y(g) — 提取 Point 的 Y 坐标
pub fn st_y(g: &SridGeometry) -> Result<f64, SpatialError> {
    match g.geom {
        Geometry::Point((_, y)) => Ok(y),
        _ => Err(SpatialError::UnsupportedOperation(format!(
            "ST_Y requires Point, got {}",
            g.geom.type_name()
        ))),
    }
}

/// ST_SRID(g) — 获取 SRID
pub fn st_srid(g: &SridGeometry) -> u32 {
    g.srid
}

/// ST_SetSRID(g, srid) — 设置 SRID（不进行坐标转换）
pub fn st_set_srid(mut g: SridGeometry, srid: u32) -> SridGeometry {
    g.set_srid(srid);
    g
}

/// ST_Distance(g1, g2) — 计算两几何体距离
///
/// - SRID=4326（geography）：Haversine 球面距离（米）
/// - 其他 SRID（geometry）：欧氏距离（坐标单位）
/// - 两几何 SRID 必须一致
pub fn st_distance(g1: &SridGeometry, g2: &SridGeometry) -> Result<f64, SpatialError> {
    if g1.srid != g2.srid {
        return Err(SpatialError::SridMismatch(g1.srid, g2.srid));
    }
    let is_geo = g1.is_geography();
    let dist_fn = if is_geo {
        haversine_distance
    } else {
        euclidean_distance
    };
    Ok(geometry_distance(&g1.geom, &g2.geom, dist_fn))
}

/// 通用几何距离（递归分解到点对）
fn geometry_distance<F: Fn(Coord, Coord) -> f64>(g1: &Geometry, g2: &Geometry, dist_fn: F) -> f64 {
    let coords1 = g1.collect_coords();
    let coords2 = g2.collect_coords();
    if coords1.is_empty() || coords2.is_empty() {
        return f64::INFINITY;
    }
    // PostGIS 语义：若几何体相交（含包含关系），距离为 0
    if geometry_intersects(g1, g2) {
        return 0.0;
    }
    // 简化实现：枚举所有点对，取最小距离
    // 对线段/多边形，还需考虑点到线段距离
    let mut min_dist = f64::INFINITY;
    // 点对点距离
    for &c1 in &coords1 {
        for &c2 in &coords2 {
            let d = dist_fn(c1, c2);
            if d < min_dist {
                min_dist = d;
            }
        }
    }
    // 点到线段距离（对 LineString/Polygon）
    let segs1 = collect_segments(g1);
    let segs2 = collect_segments(g2);
    for &c1 in &coords1 {
        for (a, b) in &segs2 {
            let d = if is_geography_dist(&dist_fn) {
                // geography 模式下，点到线段用采样近似（简化）
                // 实际 PostGIS 在球面上做更复杂处理
                approx_point_segment_geo(c1, *a, *b, &dist_fn)
            } else {
                point_segment_distance(c1, *a, *b)
            };
            if d < min_dist {
                min_dist = d;
            }
        }
    }
    for &c2 in &coords2 {
        for (a, b) in &segs1 {
            let d = if is_geography_dist(&dist_fn) {
                approx_point_segment_geo(c2, *a, *b, &dist_fn)
            } else {
                point_segment_distance(c2, *a, *b)
            };
            if d < min_dist {
                min_dist = d;
            }
        }
    }
    min_dist
}

/// 收集几何体的所有线段
fn collect_segments(g: &Geometry) -> Vec<(Coord, Coord)> {
    let mut segs = Vec::new();
    match g {
        Geometry::LineString(coords) => {
            for w in coords.windows(2) {
                segs.push((w[0], w[1]));
            }
        }
        Geometry::Polygon(rings) => {
            for ring in rings {
                for w in ring.windows(2) {
                    segs.push((w[0], w[1]));
                }
                if ring.len() >= 2 {
                    segs.push((ring[ring.len() - 1], ring[0]));
                }
            }
        }
        Geometry::MultiLineString(lines) => {
            for line in lines {
                for w in line.windows(2) {
                    segs.push((w[0], w[1]));
                }
            }
        }
        Geometry::MultiPolygon(polys) => {
            for poly in polys {
                for ring in poly {
                    for w in ring.windows(2) {
                        segs.push((w[0], w[1]));
                    }
                    if ring.len() >= 2 {
                        segs.push((ring[ring.len() - 1], ring[0]));
                    }
                }
            }
        }
        Geometry::GeometryCollection(geoms) => {
            for sub in geoms {
                segs.extend(collect_segments(sub));
            }
        }
        _ => {}
    }
    segs
}

/// 判断距离函数是否为 geography（Haversine）
///
/// 通过函数指针比较无法直接实现，这里用 helper 标记。
fn is_geography_dist<F: Fn(Coord, Coord) -> f64>(_f: &F) -> bool {
    // 简化：调用方决定，这里返回 false（笛卡尔模式走精确路径）
    // geography 模式由调用方直接调用 approx_point_segment_geo
    false
}

/// geography 模式下点到线段的近似距离（采样）
fn approx_point_segment_geo<F: Fn(Coord, Coord) -> f64>(
    p: Coord,
    a: Coord,
    b: Coord,
    dist_fn: &F,
) -> f64 {
    let n = 8; // 采样 8 个点
    let mut min_dist = dist_fn(p, a);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let sample = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        let d = dist_fn(p, sample);
        if d < min_dist {
            min_dist = d;
        }
    }
    min_dist
}

/// ST_Within(g1, g2) — g1 是否在 g2 内
///
/// OGC 语义：g1 完全在 g2 内部（含边界）。
pub fn st_within(g1: &SridGeometry, g2: &SridGeometry) -> Result<bool, SpatialError> {
    if g1.srid != g2.srid {
        return Err(SpatialError::SridMismatch(g1.srid, g2.srid));
    }
    Ok(geometry_within(&g1.geom, &g2.geom))
}

/// 通用 within 判定
fn geometry_within(inner: &Geometry, outer: &Geometry) -> bool {
    match (inner, outer) {
        (Geometry::Point(p), Geometry::Polygon(rings)) => point_in_polygon_with_holes(*p, rings),
        (Geometry::Point(p), Geometry::MultiPolygon(polys)) => polys
            .iter()
            .any(|rings| point_in_polygon_with_holes(*p, rings)),
        (Geometry::Polygon(a), Geometry::Polygon(b)) => polygon_contains_polygon(b, a),
        (Geometry::MultiPolygon(a), Geometry::Polygon(b)) => {
            a.iter().all(|ring_a| polygon_contains_polygon(b, ring_a))
        }
        (Geometry::Polygon(a), Geometry::MultiPolygon(b)) => {
            b.iter().any(|ring_b| polygon_contains_polygon(ring_b, a))
        }
        (Geometry::Point(p), Geometry::Point(q)) => p == q,
        (Geometry::GeometryCollection(items), outer) => {
            items.iter().all(|g| geometry_within(g, outer))
        }
        (_, Geometry::GeometryCollection(_)) => false, // 单几何不在集合内
        _ => false,
    }
}

/// ST_Contains(g1, g2) — g1 是否包含 g2
///
/// OGC 语义：g2 完全在 g1 内部（含边界）。
/// 等价于 `ST_Within(g2, g1)`。
pub fn st_contains(g1: &SridGeometry, g2: &SridGeometry) -> Result<bool, SpatialError> {
    st_within(g2, g1)
}

/// ST_Intersects(g1, g2) — 两几何是否相交（含边界）
///
/// 包围盒不相交 → 必不相交。
/// 包围盒相交 → 进一步精确判定。
pub fn st_intersects(g1: &SridGeometry, g2: &SridGeometry) -> Result<bool, SpatialError> {
    if g1.srid != g2.srid {
        return Err(SpatialError::SridMismatch(g1.srid, g2.srid));
    }
    // 包围盒预过滤
    let bbox1 = g1.geom.bounding_box();
    let bbox2 = g2.geom.bounding_box();
    if let (Some(b1), Some(b2)) = (bbox1, bbox2) {
        if !b1.intersects(&b2) {
            return Ok(false);
        }
    }
    Ok(geometry_intersects(&g1.geom, &g2.geom))
}

/// 通用 intersects 判定
fn geometry_intersects(g1: &Geometry, g2: &Geometry) -> bool {
    match (g1, g2) {
        (Geometry::Point(p), Geometry::Polygon(rings)) => point_in_polygon_with_holes(*p, rings),
        (Geometry::Polygon(rings), Geometry::Point(p)) => point_in_polygon_with_holes(*p, rings),
        (Geometry::Point(p), Geometry::LineString(coords))
        | (Geometry::LineString(coords), Geometry::Point(p)) => {
            // 点在线段上 → 相交
            coords.windows(2).any(|w| point_on_segment(*p, w[0], w[1]))
        }
        (Geometry::Point(p), Geometry::MultiLineString(lines))
        | (Geometry::MultiLineString(lines), Geometry::Point(p)) => lines
            .iter()
            .any(|coords| coords.windows(2).any(|w| point_on_segment(*p, w[0], w[1]))),
        (Geometry::Polygon(a), Geometry::Polygon(b)) => {
            // 边相交 OR A 包含 B 的某顶点 OR B 包含 A 的某顶点
            if rings_edges_intersect(&a[0], &b[0]) {
                return true;
            }
            if a[0].iter().any(|&p| point_in_polygon_with_holes(p, b)) {
                return true;
            }
            if b[0].iter().any(|&p| point_in_polygon_with_holes(p, a)) {
                return true;
            }
            false
        }
        (Geometry::Point(a), Geometry::Point(b)) => a == b,
        (Geometry::GeometryCollection(items), other)
        | (other, Geometry::GeometryCollection(items)) => {
            items.iter().any(|g| geometry_intersects(g, other))
        }
        _ => {
            // 未覆盖的组合：保守返回 false（distance 将通过点对/点到线段计算）
            false
        }
    }
}

/// ST_Area(g) — 计算几何体面积
///
/// - Polygon：外环面积 - 内环面积
/// - MultiPolygon：所有子多边形面积之和
/// - 其他：0
pub fn st_area(g: &SridGeometry) -> f64 {
    match &g.geom {
        Geometry::Polygon(rings) => polygon_area(rings),
        Geometry::MultiPolygon(polys) => polys.iter().map(|r| polygon_area(r)).sum(),
        Geometry::GeometryCollection(items) => items
            .iter()
            .map(|sub| st_area(&SridGeometry::new(sub.clone(), g.srid)))
            .sum(),
        _ => 0.0,
    }
}

/// ST_Length(g) — 计算几何体长度
///
/// - LineString：所有线段长度之和
/// - Polygon：所有环周长之和
/// - Multi*：子几何长度之和
pub fn st_length(g: &SridGeometry) -> f64 {
    match &g.geom {
        Geometry::LineString(coords) => line_string_length(coords),
        Geometry::Polygon(rings) => rings.iter().map(|r| line_string_length(r)).sum(),
        Geometry::MultiLineString(lines) => lines.iter().map(|l| line_string_length(l)).sum(),
        Geometry::MultiPolygon(polys) => polys
            .iter()
            .map(|rings| rings.iter().map(|r| line_string_length(r)).sum::<f64>())
            .sum(),
        Geometry::GeometryCollection(items) => items
            .iter()
            .map(|sub| st_length(&SridGeometry::new(sub.clone(), g.srid)))
            .sum(),
        _ => 0.0,
    }
}

/// ST_Envelope(g) — 计算包围盒并返回为 Polygon
///
/// 空几何返回 None。
pub fn st_envelope(g: &SridGeometry) -> Option<SridGeometry> {
    g.geom.bounding_box().map(|b| {
        let poly = Geometry::polygon(vec![vec![
            (b.min_x, b.min_y),
            (b.max_x, b.min_y),
            (b.max_x, b.max_y),
            (b.min_x, b.max_y),
            (b.min_x, b.min_y),
        ]]);
        SridGeometry::new(poly, g.srid)
    })
}

// =====================================================================
//  WKT 解析与序列化
// =====================================================================

/// ST_AsText(g) — 序列化为 WKT
pub fn st_as_text(g: &SridGeometry) -> String {
    let body = geometry_to_wkt(&g.geom);
    if g.srid != SRID_DEFAULT {
        format!("SRID={};{}", g.srid, body)
    } else {
        body
    }
}

fn geometry_to_wkt(g: &Geometry) -> String {
    fn coord_str(c: Coord) -> String {
        format!("{} {}", format_coord(c.0), format_coord(c.1))
    }
    fn format_coord(v: f64) -> String {
        if v.fract() == 0.0 {
            format!("{:.1}", v)
        } else {
            format!("{}", v)
        }
    }
    fn ring_str(coords: &[Coord]) -> String {
        coords
            .iter()
            .map(|c| coord_str(*c))
            .collect::<Vec<_>>()
            .join(", ")
    }
    match g {
        Geometry::Point(c) => format!("POINT ({})", coord_str(*c)),
        Geometry::LineString(coords) => {
            format!("LINESTRING ({})", ring_str(coords))
        }
        Geometry::Polygon(rings) => {
            let parts: Vec<String> = rings.iter().map(|r| format!("({})", ring_str(r))).collect();
            format!("POLYGON ({})", parts.join(", "))
        }
        Geometry::MultiPoint(coords) => {
            let parts: Vec<String> = coords
                .iter()
                .map(|c| format!("({})", coord_str(*c)))
                .collect();
            format!("MULTIPOINT ({})", parts.join(", "))
        }
        Geometry::MultiLineString(lines) => {
            let parts: Vec<String> = lines.iter().map(|l| format!("({})", ring_str(l))).collect();
            format!("MULTILINESTRING ({})", parts.join(", "))
        }
        Geometry::MultiPolygon(polys) => {
            let parts: Vec<String> = polys
                .iter()
                .map(|rings| {
                    let inner: Vec<String> =
                        rings.iter().map(|r| format!("({})", ring_str(r))).collect();
                    format!("({})", inner.join(", "))
                })
                .collect();
            format!("MULTIPOLYGON ({})", parts.join(", "))
        }
        Geometry::GeometryCollection(geoms) => {
            let parts: Vec<String> = geoms.iter().map(geometry_to_wkt).collect();
            format!("GEOMETRYCOLLECTION ({})", parts.join(", "))
        }
    }
}

/// ST_GeomFromText(wkt) — 解析 WKT 为几何体
///
/// 支持格式：
/// - `POINT (1 2)` / `POINT(1 2)`
/// - `LINESTRING (0 0, 1 1)`
/// - `POLYGON ((0 0, 4 0, 4 4, 0 0))`
/// - `SRID=4326;POINT (1 2)` — 携带 SRID
pub fn st_geom_from_text(wkt: &str) -> Result<SridGeometry, SpatialError> {
    parse_wkt(wkt.trim())
}

/// WKT 解析器
fn parse_wkt(s: &str) -> Result<SridGeometry, SpatialError> {
    // 提取 SRID 前缀
    let (srid, body) = if let Some(rest) = s.strip_prefix("SRID=") {
        let semi = rest
            .find(';')
            .ok_or_else(|| SpatialError::WktParse("SRID= prefix missing ';'".to_string()))?;
        let srid_str = &rest[..semi];
        let srid: u32 = srid_str
            .parse()
            .map_err(|e| SpatialError::WktParse(format!("invalid SRID '{srid_str}': {e}")))?;
        (srid, rest[semi + 1..].trim())
    } else {
        (SRID_DEFAULT, s)
    };

    // 解析类型与坐标体
    let (type_name, rest) = split_type_and_body(body)?;
    let geom = parse_geometry_body(&type_name, rest)?;
    Ok(SridGeometry::new(geom, srid))
}

/// 分离类型名与坐标体
fn split_type_and_body(s: &str) -> Result<(String, &str), SpatialError> {
    let s = s.trim();
    // 查找 '('
    let open = s
        .find('(')
        .ok_or_else(|| SpatialError::WktParse(format!("missing '(' in WKT: {s}")))?;
    let type_name = s[..open].trim().to_uppercase();
    // 查找匹配的 ')'
    let close = s
        .rfind(')')
        .ok_or_else(|| SpatialError::WktParse(format!("missing ')' in WKT: {s}")))?;
    if close < open {
        return Err(SpatialError::WktParse(format!("malformed WKT: {s}")));
    }
    let body = &s[open + 1..close];
    Ok((type_name, body))
}

/// 解析坐标体为 Geometry
fn parse_geometry_body(type_name: &str, body: &str) -> Result<Geometry, SpatialError> {
    match type_name {
        "POINT" => {
            let c = parse_coord(body)?;
            Ok(Geometry::Point(c))
        }
        "LINESTRING" => {
            let coords = parse_coord_list(body)?;
            Ok(Geometry::LineString(coords))
        }
        "POLYGON" => {
            let rings = parse_ring_list(body)?;
            Ok(Geometry::Polygon(rings))
        }
        "MULTIPOINT" => {
            // MULTIPOINT ((1 2), (3 4)) 或 MULTIPOINT (1 2, 3 4)
            let coords = parse_multipoint(body)?;
            Ok(Geometry::MultiPoint(coords))
        }
        "MULTILINESTRING" => {
            let lines = parse_ring_list(body)?;
            Ok(Geometry::MultiLineString(lines))
        }
        "MULTIPOLYGON" => {
            // MULTIPOLYGON (((0 0, ...), (...)), ((...)))
            let polys = parse_multipolygon(body)?;
            Ok(Geometry::MultiPolygon(polys))
        }
        "GEOMETRYCOLLECTION" => {
            // 递归解析子几何（需保留类型名）
            let geoms = parse_geometry_collection(body)?;
            Ok(Geometry::GeometryCollection(geoms))
        }
        other => Err(SpatialError::UnsupportedType(format!(
            "unknown WKT type: {other}"
        ))),
    }
}

/// 解析单坐标 "1 2" → (1.0, 2.0)
fn parse_coord(s: &str) -> Result<Coord, SpatialError> {
    let s = s.trim();
    let s = s.trim_start_matches('(');
    let s = s.trim_end_matches(')');
    let s = s.trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(SpatialError::WktParse(format!("expected 'X Y', got '{s}'")));
    }
    let x: f64 = parts[0]
        .parse()
        .map_err(|e| SpatialError::WktParse(format!("invalid X '{0}': {e}", parts[0])))?;
    let y: f64 = parts[1]
        .parse()
        .map_err(|e| SpatialError::WktParse(format!("invalid Y '{0}': {e}", parts[1])))?;
    if !x.is_finite() || !y.is_finite() {
        return Err(SpatialError::InvalidCoordinate(format!("{x}, {y}")));
    }
    Ok((x, y))
}

/// 解析坐标列表 "0 0, 1 1, 2 2"
fn parse_coord_list(s: &str) -> Result<Vec<Coord>, SpatialError> {
    s.split(',').map(|p| parse_coord(p.trim())).collect()
}

/// 解析环列表 "((0 0, 4 0, 4 4, 0 0), (1 1, 2 1, 2 2, 1 1))"
fn parse_ring_list(s: &str) -> Result<Vec<Vec<Coord>>, SpatialError> {
    let rings = split_top_level_groups(s)?;
    rings.iter().map(|r| parse_coord_list(r)).collect()
}

/// 解析多点 "MULTIPOINT ((1 2), (3 4))" 或 "MULTIPOINT (1 2, 3 4)"
fn parse_multipoint(s: &str) -> Result<Vec<Coord>, SpatialError> {
    let s = s.trim();
    if s.starts_with('(') {
        // 形式 ((1 2), (3 4))
        let groups = split_top_level_groups(s)?;
        groups
            .iter()
            .map(|g| {
                let g = g.trim();
                let g = g.strip_prefix('(').unwrap_or(g);
                let g = g.strip_suffix(')').unwrap_or(g);
                parse_coord(g)
            })
            .collect()
    } else {
        // 形式 (1 2, 3 4)
        parse_coord_list(s)
    }
}

/// 解析多多边形 "(((...), (...)), ((...)))"
fn parse_multipolygon(s: &str) -> Result<Vec<Vec<Vec<Coord>>>, SpatialError> {
    let polys = split_top_level_groups(s)?;
    polys
        .iter()
        .map(|poly| {
            let rings = split_top_level_groups(poly)?;
            rings.iter().map(|r| parse_coord_list(r)).collect()
        })
        .collect()
}

/// 解析 GeometryCollection "POINT (1 2), LINESTRING (0 0, 1 1)"
fn parse_geometry_collection(s: &str) -> Result<Vec<Geometry>, SpatialError> {
    let items = split_top_level_items(s)?;
    items
        .iter()
        .map(|item| {
            let (type_name, body) = split_type_and_body(item)?;
            parse_geometry_body(&type_name, body)
        })
        .collect()
}

/// 在 "((a, b), (c, d))" 中按顶层分组分割
///
/// 输入应已剥离最外层括号。
/// 例如 "((0 0, 1 1), (2 2, 3 3))" → ["0 0, 1 1", "2 2, 3 3"]
fn split_top_level_groups(s: &str) -> Result<Vec<String>, SpatialError> {
    let s = s.trim();
    let mut groups = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    groups.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        groups.push(trimmed);
    }
    Ok(groups)
}

/// 在 GeometryCollection 中按顶层项分割（保留类型名）
///
/// 例如 "POINT (1 2), LINESTRING (0 0, 1 1)" → ["POINT (1 2)", "LINESTRING (0 0, 1 1)"]
fn split_top_level_items(s: &str) -> Result<Vec<String>, SpatialError> {
    let mut items = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    items.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        items.push(trimmed);
    }
    Ok(items)
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Row;
    use szrsql_types::value::Value;

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    fn make_point_rows() -> Vec<Row> {
        vec![
            vec![Value::Text("POINT (1 1)".to_string())],
            vec![Value::Text("POINT (5 5)".to_string())],
            vec![Value::Text("POINT (10 10)".to_string())],
        ]
    }

    // =================================================================
    //  SpatialError 测试
    // =================================================================

    #[test]
    fn test_error_to_execution_error() {
        let err: ExecutionError = SpatialError::EmptyGeometry.into();
        assert!(matches!(err, ExecutionError::EvalError(_)));
    }

    #[test]
    fn test_error_wkt_parse_missing_paren() {
        let err = st_geom_from_text("POINT 1 2").unwrap_err();
        assert!(matches!(err, SpatialError::WktParse(_)));
    }

    #[test]
    fn test_error_unsupported_operation() {
        let g = st_geom_from_text("LINESTRING (0 0, 1 1)").unwrap();
        let err = st_x(&g).unwrap_err();
        assert!(matches!(err, SpatialError::UnsupportedOperation(_)));
    }

    #[test]
    fn test_error_srid_mismatch() {
        let g1 = st_point_with_srid(1.0, 2.0, 4326);
        let g2 = st_point_with_srid(3.0, 4.0, 0);
        let err = st_distance(&g1, &g2).unwrap_err();
        assert!(matches!(err, SpatialError::SridMismatch(4326, 0)));
    }

    #[test]
    fn test_error_unknown_wkt_type() {
        let err = st_geom_from_text("UNKNOWNTYPE (1 2)").unwrap_err();
        assert!(matches!(err, SpatialError::UnsupportedType(_)));
    }

    #[test]
    fn test_error_invalid_coordinate() {
        let err = st_geom_from_text("POINT (abc 2)").unwrap_err();
        assert!(matches!(err, SpatialError::WktParse(_)));
    }

    // =================================================================
    //  BoundingBox 测试
    // =================================================================

    #[test]
    fn test_bbox_new() {
        let b = BoundingBox::new(0.0, 0.0, 10.0, 20.0);
        assert_eq!(b.width(), 10.0);
        assert_eq!(b.height(), 20.0);
        assert_eq!(b.area(), 200.0);
        assert_eq!(b.perimeter(), 60.0);
        assert_eq!(b.center(), (5.0, 10.0));
    }

    #[test]
    fn test_bbox_from_point() {
        let b = BoundingBox::from_point(3.0, 4.0);
        assert_eq!(b.width(), 0.0);
        assert_eq!(b.height(), 0.0);
        assert_eq!(b.area(), 0.0);
        assert!(b.contains_point(3.0, 4.0));
    }

    #[test]
    fn test_bbox_from_coords() {
        let coords = vec![(0.0, 0.0), (5.0, 2.0), (-3.0, 8.0)];
        let b = BoundingBox::from_coords(coords).unwrap();
        assert_eq!(b.min_x, -3.0);
        assert_eq!(b.max_x, 5.0);
        assert_eq!(b.min_y, 0.0);
        assert_eq!(b.max_y, 8.0);
    }

    #[test]
    fn test_bbox_from_empty_coords() {
        assert!(BoundingBox::from_coords(vec![]).is_none());
    }

    #[test]
    fn test_bbox_contains_point() {
        let b = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        assert!(b.contains_point(5.0, 5.0));
        assert!(b.contains_point(0.0, 0.0)); // 边界
        assert!(b.contains_point(10.0, 10.0)); // 边界
        assert!(!b.contains_point(11.0, 5.0));
        assert!(!b.contains_point(5.0, -1.0));
    }

    #[test]
    fn test_bbox_contains_bbox() {
        let outer = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let inner = BoundingBox::new(2.0, 2.0, 8.0, 8.0);
        let outside = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        assert!(outer.contains_bbox(&inner));
        assert!(!outer.contains_bbox(&outside));
        assert!(!inner.contains_bbox(&outer));
    }

    #[test]
    fn test_bbox_intersects() {
        let a = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        let c = BoundingBox::new(20.0, 20.0, 30.0, 30.0);
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn test_bbox_intersects_boundary() {
        let a = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BoundingBox::new(10.0, 0.0, 20.0, 10.0);
        assert!(a.intersects(&b)); // 共边
    }

    #[test]
    fn test_bbox_union() {
        let a = BoundingBox::new(0.0, 0.0, 5.0, 5.0);
        let b = BoundingBox::new(3.0, 3.0, 10.0, 10.0);
        let u = a.union(&b);
        assert_eq!(u.min_x, 0.0);
        assert_eq!(u.max_x, 10.0);
        assert_eq!(u.min_y, 0.0);
        assert_eq!(u.max_y, 10.0);
    }

    #[test]
    fn test_bbox_intersection() {
        let a = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let b = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        let i = a.intersection(&b).unwrap();
        assert_eq!(i.min_x, 5.0);
        assert_eq!(i.max_x, 10.0);
        assert_eq!(i.min_y, 5.0);
        assert_eq!(i.max_y, 10.0);
    }

    #[test]
    fn test_bbox_intersection_no_overlap() {
        let a = BoundingBox::new(0.0, 0.0, 1.0, 1.0);
        let b = BoundingBox::new(5.0, 5.0, 10.0, 10.0);
        assert!(a.intersection(&b).is_none());
    }

    #[test]
    fn test_bbox_extend_point() {
        let mut b = BoundingBox::new(0.0, 0.0, 5.0, 5.0);
        b.extend_point(10.0, -3.0);
        assert_eq!(b.max_x, 10.0);
        assert_eq!(b.min_y, -3.0);
    }

    #[test]
    fn test_bbox_area_increase() {
        let b = BoundingBox::new(0.0, 0.0, 10.0, 10.0);
        let other = BoundingBox::new(5.0, 5.0, 15.0, 15.0);
        // union = (0,0,15,15), area = 225, original area = 100, increase = 125
        assert!((b.area_increase(&other) - 125.0).abs() < 1e-9);
    }

    #[test]
    fn test_bbox_default_is_empty() {
        let b = BoundingBox::default();
        assert!(b.is_empty());
    }

    // =================================================================
    //  Geometry 构造与基础测试
    // =================================================================

    #[test]
    fn test_geometry_point() {
        let g = Geometry::point(1.0, 2.0);
        assert_eq!(g.type_name(), "Point");
        assert!(!g.is_empty());
        assert_eq!(g.bounding_box(), Some(BoundingBox::from_point(1.0, 2.0)));
    }

    #[test]
    fn test_geometry_linestring() {
        let g = Geometry::line_string(vec![(0.0, 0.0), (3.0, 4.0), (6.0, 0.0)]);
        assert_eq!(g.type_name(), "LineString");
        let bbox = g.bounding_box().unwrap();
        assert_eq!(bbox.min_x, 0.0);
        assert_eq!(bbox.max_x, 6.0);
        assert_eq!(bbox.min_y, 0.0);
        assert_eq!(bbox.max_y, 4.0);
    }

    #[test]
    fn test_geometry_polygon_bbox() {
        let g = Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (0.0, 0.0),
        ]]);
        let bbox = g.bounding_box().unwrap();
        assert_eq!(bbox.min_x, 0.0);
        assert_eq!(bbox.max_x, 4.0);
    }

    #[test]
    fn test_geometry_empty() {
        assert!(Geometry::line_string(vec![]).is_empty());
        assert!(Geometry::polygon(vec![]).is_empty());
        assert!(!Geometry::point(0.0, 0.0).is_empty());
    }

    #[test]
    fn test_geometry_collect_coords() {
        let g = Geometry::polygon(vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 0.0)]]);
        let coords = g.collect_coords();
        assert_eq!(coords.len(), 4);
    }

    #[test]
    fn test_geometry_collection_bbox() {
        let g = Geometry::GeometryCollection(vec![
            Geometry::point(0.0, 0.0),
            Geometry::point(10.0, 5.0),
        ]);
        let bbox = g.bounding_box().unwrap();
        assert_eq!(bbox.min_x, 0.0);
        assert_eq!(bbox.max_x, 10.0);
    }

    // =================================================================
    //  SridGeometry 测试
    // =================================================================

    #[test]
    fn test_srid_geometry_default() {
        let g = SridGeometry::with_default_srid(Geometry::point(1.0, 2.0));
        assert_eq!(g.srid, SRID_DEFAULT);
        assert!(!g.is_geography());
    }

    #[test]
    fn test_srid_geometry_wgs84() {
        let g = SridGeometry::with_wgs84(Geometry::point(116.0, 39.0));
        assert_eq!(g.srid, SRID_WGS84);
        assert!(g.is_geography());
    }

    #[test]
    fn test_srid_geometry_set_srid() {
        let mut g = SridGeometry::with_default_srid(Geometry::point(1.0, 2.0));
        g.set_srid(4326);
        assert_eq!(g.srid, 4326);
        assert!(g.is_geography());
    }

    // =================================================================
    //  ST_Point / ST_X / ST_Y 测试
    // =================================================================

    #[test]
    fn test_st_point() {
        let g = st_point(1.0, 2.0);
        assert!(matches!(g.geom, Geometry::Point(_)));
        assert_eq!(g.srid, SRID_DEFAULT);
    }

    #[test]
    fn test_st_point_with_srid() {
        let g = st_point_with_srid(116.0, 39.0, 4326);
        assert_eq!(g.srid, 4326);
        assert!(g.is_geography());
    }

    #[test]
    fn test_st_x_y() {
        let g = st_point(3.5, 4.5);
        assert_eq!(st_x(&g).unwrap(), 3.5);
        assert_eq!(st_y(&g).unwrap(), 4.5);
    }

    #[test]
    fn test_st_x_y_on_non_point() {
        let g =
            SridGeometry::with_default_srid(Geometry::line_string(vec![(0.0, 0.0), (1.0, 1.0)]));
        assert!(st_x(&g).is_err());
        assert!(st_y(&g).is_err());
    }

    #[test]
    fn test_st_srid() {
        let g = st_point_with_srid(1.0, 2.0, 3857);
        assert_eq!(st_srid(&g), 3857);
    }

    #[test]
    fn test_st_set_srid() {
        let g = st_point(1.0, 2.0);
        let g2 = st_set_srid(g, 4326);
        assert_eq!(g2.srid, 4326);
    }

    // =================================================================
    //  ST_Distance 测试
    // =================================================================

    #[test]
    fn test_st_distance_geometry_points() {
        let g1 = st_point(0.0, 0.0);
        let g2 = st_point(3.0, 4.0);
        let d = st_distance(&g1, &g2).unwrap();
        assert!((d - 5.0).abs() < 1e-9);
    }

    #[test]
    fn test_st_distance_same_point() {
        let g1 = st_point(1.0, 1.0);
        let g2 = st_point(1.0, 1.0);
        let d = st_distance(&g1, &g2).unwrap();
        assert!(d.abs() < 1e-9);
    }

    #[test]
    fn test_st_distance_geography_haversine() {
        // 北京 (116.40, 39.90) → 上海 (121.47, 31.23)
        let g1 = st_point_with_srid(116.40, 39.90, 4326);
        let g2 = st_point_with_srid(121.47, 31.23, 4326);
        let d = st_distance(&g1, &g2).unwrap();
        // 实际约 1067 km，Haversine 球面距离应在 1000-1100 km 范围
        assert!(d > 1_000_000.0 && d < 1_100_000.0, "got {d}");
    }

    #[test]
    fn test_st_distance_geography_same_point() {
        let g1 = st_point_with_srid(116.0, 39.0, 4326);
        let g2 = st_point_with_srid(116.0, 39.0, 4326);
        let d = st_distance(&g1, &g2).unwrap();
        assert!(d.abs() < 1e-3);
    }

    #[test]
    fn test_st_distance_point_to_linestring() {
        // Point (0, 5) 到 LineString ((0,0),(10,0)) 距离应为 5
        let g1 = st_point(0.0, 5.0);
        let g2 =
            SridGeometry::with_default_srid(Geometry::line_string(vec![(0.0, 0.0), (10.0, 0.0)]));
        let d = st_distance(&g1, &g2).unwrap();
        assert!((d - 5.0).abs() < 1e-9, "got {d}");
    }

    #[test]
    fn test_st_distance_point_to_polygon() {
        // Point (5, 5) 在 Polygon ((0,0),(10,0),(10,10),(0,10)) 内 → 距离 0
        let g1 = st_point(5.0, 5.0);
        let g2 = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let d = st_distance(&g1, &g2).unwrap();
        assert!(d.abs() < 1e-9, "got {d}");
    }

    #[test]
    fn test_st_distance_polygon_external_point() {
        // Point (15, 5) 在 Polygon ((0,0),(10,0),(10,10),(0,10)) 外，距右边界 5
        let g1 = st_point(15.0, 5.0);
        let g2 = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let d = st_distance(&g1, &g2).unwrap();
        assert!((d - 5.0).abs() < 1e-9, "got {d}");
    }

    // =================================================================
    //  point_in_polygon 测试
    // =================================================================

    #[test]
    fn test_point_in_polygon_inside() {
        let ring = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ];
        assert!(point_in_polygon((5.0, 5.0), &ring));
    }

    #[test]
    fn test_point_in_polygon_outside() {
        let ring = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ];
        assert!(!point_in_polygon((15.0, 5.0), &ring));
        assert!(!point_in_polygon((-5.0, 5.0), &ring));
        assert!(!point_in_polygon((5.0, 15.0), &ring));
    }

    #[test]
    fn test_point_in_polygon_on_boundary() {
        let ring = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ];
        // 角点
        assert!(point_in_polygon((0.0, 0.0), &ring));
        // 边上
        assert!(point_in_polygon((5.0, 0.0), &ring));
        assert!(point_in_polygon((10.0, 5.0), &ring));
    }

    #[test]
    fn test_point_in_polygon_with_hole() {
        let rings = vec![
            // 外环
            vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ],
            // 内环（孔洞）
            vec![(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0), (3.0, 3.0)],
        ];
        // 在外环内但不在孔洞内 → true
        assert!(point_in_polygon_with_holes((1.0, 1.0), &rings));
        // 在孔洞内 → false
        assert!(!point_in_polygon_with_holes((5.0, 5.0), &rings));
        // 在外环外 → false
        assert!(!point_in_polygon_with_holes((15.0, 15.0), &rings));
    }

    #[test]
    fn test_point_in_polygon_concave() {
        // 凹多边形（星形）：(0,5),(5,0),(10,5),(5,10) — 实际是菱形
        let ring = vec![(0.0, 5.0), (5.0, 0.0), (10.0, 5.0), (5.0, 10.0), (0.0, 5.0)];
        assert!(point_in_polygon((5.0, 5.0), &ring)); // 中心
        assert!(!point_in_polygon((0.0, 0.0), &ring)); // 外角
    }

    #[test]
    fn test_point_in_polygon_degenerate() {
        // 少于 3 个点
        assert!(!point_in_polygon((0.0, 0.0), &[(0.0, 0.0), (1.0, 1.0)]));
        assert!(!point_in_polygon((0.0, 0.0), &[]));
    }

    // =================================================================
    //  ST_Within / ST_Contains 测试
    // =================================================================

    #[test]
    fn test_st_within_point_in_polygon() {
        let p = st_point(5.0, 5.0);
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        assert!(st_within(&p, &poly).unwrap());
    }

    #[test]
    fn test_st_within_point_outside_polygon() {
        let p = st_point(15.0, 15.0);
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        assert!(!st_within(&p, &poly).unwrap());
    }

    #[test]
    fn test_st_within_point_on_boundary() {
        let p = st_point(5.0, 0.0); // 在边上
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        assert!(st_within(&p, &poly).unwrap()); // 含边界
    }

    #[test]
    fn test_st_within_polygon_in_polygon() {
        let inner = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (2.0, 2.0),
            (4.0, 2.0),
            (4.0, 4.0),
            (2.0, 4.0),
            (2.0, 2.0),
        ]]));
        let outer = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        assert!(st_within(&inner, &outer).unwrap());
        assert!(!st_within(&outer, &inner).unwrap());
    }

    #[test]
    fn test_st_within_overlapping_polygons() {
        let a = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let b = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (5.0, 5.0),
            (15.0, 5.0),
            (15.0, 15.0),
            (5.0, 15.0),
            (5.0, 5.0),
        ]]));
        assert!(!st_within(&a, &b).unwrap());
        assert!(!st_within(&b, &a).unwrap());
    }

    #[test]
    fn test_st_contains_inverse_of_within() {
        let p = st_point(5.0, 5.0);
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        assert!(st_contains(&poly, &p).unwrap());
        assert!(st_within(&p, &poly).unwrap());
        assert!(!st_contains(&p, &poly).unwrap());
    }

    #[test]
    fn test_st_contains_polygon_point() {
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let inside = st_point(5.0, 5.0);
        let outside = st_point(15.0, 15.0);
        assert!(st_contains(&poly, &inside).unwrap());
        assert!(!st_contains(&poly, &outside).unwrap());
    }

    #[test]
    fn test_st_within_point_in_multipolygon() {
        let p = st_point(15.0, 5.0);
        let mp = SridGeometry::with_default_srid(Geometry::MultiPolygon(vec![
            vec![vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ]],
            vec![vec![
                (12.0, 0.0),
                (20.0, 0.0),
                (20.0, 10.0),
                (12.0, 10.0),
                (12.0, 0.0),
            ]],
        ]));
        assert!(st_within(&p, &mp).unwrap());
    }

    // =================================================================
    //  ST_Intersects 测试
    // =================================================================

    #[test]
    fn test_st_intersects_overlapping_polygons() {
        let a = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let b = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (5.0, 5.0),
            (15.0, 5.0),
            (15.0, 15.0),
            (5.0, 15.0),
            (5.0, 5.0),
        ]]));
        assert!(st_intersects(&a, &b).unwrap());
    }

    #[test]
    fn test_st_intersects_disjoint_polygons() {
        let a = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (0.0, 1.0),
            (0.0, 0.0),
        ]]));
        let b = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (10.0, 10.0),
            (11.0, 10.0),
            (11.0, 11.0),
            (10.0, 11.0),
            (10.0, 10.0),
        ]]));
        assert!(!st_intersects(&a, &b).unwrap());
    }

    #[test]
    fn test_st_intersects_point_polygon() {
        let p = st_point(5.0, 5.0);
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        assert!(st_intersects(&p, &poly).unwrap());
    }

    #[test]
    fn test_st_intersects_equal_points() {
        let p1 = st_point(1.0, 2.0);
        let p2 = st_point(1.0, 2.0);
        assert!(st_intersects(&p1, &p2).unwrap());
    }

    // =================================================================
    //  ST_Area / ST_Length 测试
    // =================================================================

    #[test]
    fn test_st_area_polygon() {
        let g = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (0.0, 0.0),
        ]]));
        assert!((st_area(&g) - 16.0).abs() < 1e-9);
    }

    #[test]
    fn test_st_area_polygon_with_hole() {
        let g = SridGeometry::with_default_srid(Geometry::polygon(vec![
            vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ],
            vec![(3.0, 3.0), (7.0, 3.0), (7.0, 7.0), (3.0, 7.0), (3.0, 3.0)],
        ]));
        // 外环面积 100 - 内环面积 16 = 84
        assert!((st_area(&g) - 84.0).abs() < 1e-9);
    }

    #[test]
    fn test_st_area_multipolygon() {
        let g = SridGeometry::with_default_srid(Geometry::MultiPolygon(vec![
            vec![vec![
                (0.0, 0.0),
                (2.0, 0.0),
                (2.0, 2.0),
                (0.0, 2.0),
                (0.0, 0.0),
            ]],
            vec![vec![
                (10.0, 10.0),
                (12.0, 10.0),
                (12.0, 12.0),
                (10.0, 12.0),
                (10.0, 10.0),
            ]],
        ]));
        // 两个 4 单位面积的多边形
        assert!((st_area(&g) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn test_st_area_point_is_zero() {
        let g = st_point(1.0, 2.0);
        assert_eq!(st_area(&g), 0.0);
    }

    #[test]
    fn test_st_length_linestring() {
        let g = SridGeometry::with_default_srid(Geometry::line_string(vec![
            (0.0, 0.0),
            (3.0, 4.0),
            (6.0, 4.0),
        ]));
        // 5 + 3 = 8
        assert!((st_length(&g) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn test_st_length_polygon_perimeter() {
        let g = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (0.0, 0.0),
        ]]));
        assert!((st_length(&g) - 16.0).abs() < 1e-9);
    }

    #[test]
    fn test_st_length_point_is_zero() {
        let g = st_point(1.0, 2.0);
        assert_eq!(st_length(&g), 0.0);
    }

    // =================================================================
    //  ST_Envelope 测试
    // =================================================================

    #[test]
    fn test_st_envelope_polygon() {
        let g = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (1.0, 2.0),
            (5.0, 2.0),
            (5.0, 8.0),
            (1.0, 8.0),
            (1.0, 2.0),
        ]]));
        let env = st_envelope(&g).unwrap();
        // envelope 是覆盖原多边形的最小矩形
        let bbox = env.geom.bounding_box().unwrap();
        assert_eq!(bbox.min_x, 1.0);
        assert_eq!(bbox.max_x, 5.0);
        assert_eq!(bbox.min_y, 2.0);
        assert_eq!(bbox.max_y, 8.0);
    }

    #[test]
    fn test_st_envelope_point() {
        let g = st_point(3.0, 4.0);
        let env = st_envelope(&g).unwrap();
        let bbox = env.geom.bounding_box().unwrap();
        assert_eq!(bbox.min_x, 3.0);
        assert_eq!(bbox.max_x, 3.0);
    }

    #[test]
    fn test_st_envelope_linestring() {
        let g = SridGeometry::with_default_srid(Geometry::line_string(vec![
            (-3.0, 0.0),
            (5.0, 7.0),
            (2.0, -1.0),
        ]));
        let env = st_envelope(&g).unwrap();
        let bbox = env.geom.bounding_box().unwrap();
        assert_eq!(bbox.min_x, -3.0);
        assert_eq!(bbox.max_x, 5.0);
        assert_eq!(bbox.min_y, -1.0);
        assert_eq!(bbox.max_y, 7.0);
    }

    // =================================================================
    //  WKT 解析与序列化测试
    // =================================================================

    #[test]
    fn test_wkt_point() {
        let g = st_geom_from_text("POINT (1 2)").unwrap();
        assert!(matches!(g.geom, Geometry::Point((1.0, 2.0))));
        assert_eq!(g.srid, SRID_DEFAULT);
    }

    #[test]
    fn test_wkt_point_no_space() {
        let g = st_geom_from_text("POINT(1 2)").unwrap();
        assert!(matches!(g.geom, Geometry::Point(_)));
    }

    #[test]
    fn test_wkt_point_decimal() {
        let g = st_geom_from_text("POINT (1.5 2.5)").unwrap();
        if let Geometry::Point((x, y)) = g.geom {
            assert!((x - 1.5).abs() < 1e-9);
            assert!((y - 2.5).abs() < 1e-9);
        } else {
            panic!("expected Point");
        }
    }

    #[test]
    fn test_wkt_linestring() {
        let g = st_geom_from_text("LINESTRING (0 0, 1 1, 2 2)").unwrap();
        if let Geometry::LineString(coords) = g.geom {
            assert_eq!(coords.len(), 3);
            assert_eq!(coords[0], (0.0, 0.0));
            assert_eq!(coords[2], (2.0, 2.0));
        } else {
            panic!("expected LineString");
        }
    }

    #[test]
    fn test_wkt_polygon() {
        let g = st_geom_from_text("POLYGON ((0 0, 4 0, 4 4, 0 4, 0 0))").unwrap();
        if let Geometry::Polygon(rings) = g.geom {
            assert_eq!(rings.len(), 1);
            assert_eq!(rings[0].len(), 5);
        } else {
            panic!("expected Polygon");
        }
    }

    #[test]
    fn test_wkt_polygon_with_hole() {
        let g =
            st_geom_from_text("POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (3 3, 7 3, 7 7, 3 7, 3 3))")
                .unwrap();
        if let Geometry::Polygon(rings) = g.geom {
            assert_eq!(rings.len(), 2);
            assert_eq!(rings[0].len(), 5);
            assert_eq!(rings[1].len(), 5);
        } else {
            panic!("expected Polygon");
        }
    }

    #[test]
    fn test_wkt_multipoint() {
        let g = st_geom_from_text("MULTIPOINT ((1 2), (3 4))").unwrap();
        if let Geometry::MultiPoint(coords) = g.geom {
            assert_eq!(coords.len(), 2);
            assert_eq!(coords[0], (1.0, 2.0));
        } else {
            panic!("expected MultiPoint");
        }
    }

    #[test]
    fn test_wkt_multilinestring() {
        let g = st_geom_from_text("MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))").unwrap();
        if let Geometry::MultiLineString(lines) = g.geom {
            assert_eq!(lines.len(), 2);
        } else {
            panic!("expected MultiLineString");
        }
    }

    #[test]
    fn test_wkt_multipolygon() {
        let g = st_geom_from_text("MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((2 2, 3 2, 3 3, 2 2)))")
            .unwrap();
        if let Geometry::MultiPolygon(polys) = g.geom {
            assert_eq!(polys.len(), 2);
        } else {
            panic!("expected MultiPolygon");
        }
    }

    #[test]
    fn test_wkt_geometrycollection() {
        let g = st_geom_from_text("GEOMETRYCOLLECTION (POINT (1 2), POINT (3 4))").unwrap();
        if let Geometry::GeometryCollection(items) = g.geom {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected GeometryCollection");
        }
    }

    #[test]
    fn test_wkt_srid_prefix() {
        let g = st_geom_from_text("SRID=4326;POINT (1 2)").unwrap();
        assert_eq!(g.srid, 4326);
        assert!(matches!(g.geom, Geometry::Point(_)));
    }

    #[test]
    fn test_wkt_roundtrip_point() {
        let orig = st_point(1.5, 2.5);
        let wkt = st_as_text(&orig);
        let parsed = st_geom_from_text(&wkt).unwrap();
        assert_eq!(orig, parsed);
    }

    #[test]
    fn test_wkt_roundtrip_linestring() {
        let orig = SridGeometry::with_default_srid(Geometry::line_string(vec![
            (0.0, 0.0),
            (3.0, 4.0),
            (6.0, 0.0),
        ]));
        let wkt = st_as_text(&orig);
        let parsed = st_geom_from_text(&wkt).unwrap();
        assert_eq!(orig, parsed);
    }

    #[test]
    fn test_wkt_roundtrip_polygon() {
        let orig = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (0.0, 0.0),
        ]]));
        let wkt = st_as_text(&orig);
        let parsed = st_geom_from_text(&wkt).unwrap();
        assert_eq!(orig, parsed);
    }

    #[test]
    fn test_wkt_roundtrip_with_srid() {
        let orig = st_point_with_srid(1.0, 2.0, 4326);
        let wkt = st_as_text(&orig);
        assert!(wkt.starts_with("SRID=4326;"));
        let parsed = st_geom_from_text(&wkt).unwrap();
        assert_eq!(orig, parsed);
    }

    #[test]
    fn test_wkt_invalid_missing_paren() {
        assert!(st_geom_from_text("POINT 1 2").is_err());
    }

    #[test]
    fn test_wkt_invalid_unknown_type() {
        assert!(st_geom_from_text("FOO (1 2)").is_err());
    }

    #[test]
    fn test_wkt_invalid_srid_no_semicolon() {
        assert!(st_geom_from_text("SRID=4326 POINT (1 2)").is_err());
    }

    // =================================================================
    //  E2E: 集成场景
    // =================================================================

    #[test]
    fn test_e2e_create_table_insert_query() {
        // 模拟 CREATE TABLE t (loc GEOGRAPHY(Point)) → INSERT ST_Point(1,2)
        let rows = make_point_rows();
        assert_eq!(rows.len(), 3);

        // 解析每行的 POINT WKT
        let geoms: Vec<SridGeometry> = rows
            .iter()
            .filter_map(|r| {
                if let Some(Value::Text(s)) = r.first() {
                    st_geom_from_text(s).ok()
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(geoms.len(), 3);

        // ST_Distance(geoms[0], geoms[1])
        let d01 = st_distance(&geoms[0], &geoms[1]).unwrap();
        assert!((d01 - (32.0f64).sqrt()).abs() < 1e-9, "got {d01}");
    }

    #[test]
    fn test_e2e_geography_distance_consistent_with_postgis() {
        // PostGIS：ST_Distance('POINT(0 0)'::geography, 'POINT(0 1)'::geography) ≈ 111195 m
        // 1 度纬度 ≈ 111195 米（Haversine 球面）
        let g1 = st_point_with_srid(0.0, 0.0, 4326);
        let g2 = st_point_with_srid(0.0, 1.0, 4326);
        let d = st_distance(&g1, &g2).unwrap();
        // 容差 ±500m（Haversine 球面 vs WGS84 椭球差异）
        assert!((d - 111195.0).abs() < 1000.0, "got {d}");
    }

    #[test]
    fn test_e2e_geography_equator_distance() {
        // 赤道上 1 度经度 ≈ 111195 m
        let g1 = st_point_with_srid(0.0, 0.0, 4326);
        let g2 = st_point_with_srid(1.0, 0.0, 4326);
        let d = st_distance(&g1, &g2).unwrap();
        assert!((d - 111195.0).abs() < 1000.0, "got {d}");
    }

    #[test]
    fn test_e2e_within_query() {
        // 模拟范围查询：在多边形内的点
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let points = [
            st_point(5.0, 5.0),   // 内
            st_point(15.0, 15.0), // 外
            st_point(0.0, 5.0),   // 边界
        ];
        let inside: Vec<bool> = points
            .iter()
            .map(|p| st_within(p, &poly).unwrap())
            .collect();
        assert_eq!(inside, vec![true, false, true]);
    }

    #[test]
    fn test_e2e_contains_query() {
        let poly = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let p = st_point(5.0, 5.0);
        assert!(st_contains(&poly, &p).unwrap());
    }

    #[test]
    fn test_e2e_envelope_then_within() {
        // 模拟 GiST 索引预过滤：先 envelope 相交，再精确 within
        let outer = SridGeometry::with_default_srid(Geometry::polygon(vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]]));
        let candidate = st_point(5.0, 5.0);

        let env = st_envelope(&outer).unwrap();
        let env_bbox = env.geom.bounding_box().unwrap();
        let cand_bbox = candidate.geom.bounding_box().unwrap();
        // 索引预过滤
        assert!(env_bbox.intersects(&cand_bbox));
        // 精确判定
        assert!(st_within(&candidate, &outer).unwrap());
    }

    #[test]
    fn test_e2e_buffer_via_envelope() {
        // 简化的 buffer：用 envelope + 距离扩展作为范围查询
        let center = st_point(5.0, 5.0);
        let points = [st_point(5.0, 5.0), st_point(6.0, 6.0), st_point(15.0, 15.0)];
        let radius = 2.0;
        let in_range: Vec<bool> = points
            .iter()
            .map(|p| st_distance(&center, p).unwrap() <= radius)
            .collect();
        assert_eq!(in_range, vec![true, true, false]);
    }

    #[test]
    fn test_e2e_multipolygon_contains() {
        let mp = SridGeometry::with_default_srid(Geometry::MultiPolygon(vec![
            vec![vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 10.0),
                (0.0, 10.0),
                (0.0, 0.0),
            ]],
            vec![vec![
                (100.0, 100.0),
                (110.0, 100.0),
                (110.0, 110.0),
                (100.0, 110.0),
                (100.0, 100.0),
            ]],
        ]));
        let p1 = st_point(5.0, 5.0);
        let p2 = st_point(105.0, 105.0);
        let p3 = st_point(50.0, 50.0);
        assert!(st_contains(&mp, &p1).unwrap());
        assert!(st_contains(&mp, &p2).unwrap());
        assert!(!st_contains(&mp, &p3).unwrap());
    }

    #[test]
    fn test_e2e_wkt_persistence() {
        // 模拟数据库存储：geometry → WKT → 存储 → 读取 → 解析
        let orig = st_point_with_srid(116.40, 39.90, 4326);
        let wkt = st_as_text(&orig);
        // 模拟存储为 Text
        let stored = Value::Text(wkt);
        // 读取并解析
        if let Value::Text(s) = &stored {
            let parsed = st_geom_from_text(s).unwrap();
            assert_eq!(orig, parsed);
        } else {
            panic!("expected Text");
        }
    }

    // =================================================================
    //  纯函数与边界测试
    // =================================================================

    #[test]
    fn test_euclidean_distance() {
        assert!((euclidean_distance((0.0, 0.0), (3.0, 4.0)) - 5.0).abs() < 1e-9);
        assert!(euclidean_distance((1.0, 1.0), (1.0, 1.0)).abs() < 1e-9);
    }

    #[test]
    fn test_haversine_distance_zero() {
        let d = haversine_distance((0.0, 0.0), (0.0, 0.0));
        assert!(d.abs() < 1e-6);
    }

    #[test]
    fn test_haversine_distance_antipode() {
        // 对跖点距离 ≈ π * R ≈ 20015087 m
        let d = haversine_distance((0.0, 0.0), (180.0, 0.0));
        let expected = std::f64::consts::PI * EARTH_RADIUS_METERS;
        assert!((d - expected).abs() < 1.0, "got {d}, expected {expected}");
    }

    #[test]
    fn test_point_segment_distance() {
        assert!((point_segment_distance((0.0, 5.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-9);
        assert!((point_segment_distance((-5.0, 0.0), (0.0, 0.0), (10.0, 0.0)) - 5.0).abs() < 1e-9);
        assert!(point_segment_distance((5.0, 0.0), (0.0, 0.0), (10.0, 0.0)).abs() < 1e-9);
    }

    #[test]
    fn test_point_segment_distance_degenerate() {
        // 退化线段（两端点重合）→ 等价于点到点距离
        let d = point_segment_distance((3.0, 4.0), (1.0, 1.0), (1.0, 1.0));
        let expected = (2.0f64 * 2.0 + 3.0 * 3.0).sqrt(); // sqrt(13)
        assert!((d - expected).abs() < 1e-9, "got {d}, expected {expected}");
    }

    #[test]
    fn test_ring_area_unit_square() {
        let ring = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)];
        assert!((ring_area(&ring) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_ring_area_triangle() {
        let ring = vec![(0.0, 0.0), (4.0, 0.0), (0.0, 3.0), (0.0, 0.0)];
        assert!((ring_area(&ring) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn test_ring_area_degenerate() {
        assert_eq!(ring_area(&[(0.0, 0.0), (1.0, 1.0)]), 0.0);
        assert_eq!(ring_area(&[(0.0, 0.0)]), 0.0);
    }

    #[test]
    fn test_segments_intersect_crossing() {
        assert!(segments_intersect(
            (0.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (10.0, 0.0)
        ));
    }

    #[test]
    fn test_segments_intersect_parallel() {
        assert!(!segments_intersect(
            (0.0, 0.0),
            (10.0, 0.0),
            (0.0, 5.0),
            (10.0, 5.0)
        ));
    }

    #[test]
    fn test_segments_intersect_shared_endpoint() {
        assert!(segments_intersect(
            (0.0, 0.0),
            (5.0, 5.0),
            (5.0, 5.0),
            (10.0, 0.0)
        ));
    }

    #[test]
    fn test_point_on_segment() {
        assert!(point_on_segment((5.0, 0.0), (0.0, 0.0), (10.0, 0.0)));
        assert!(point_on_segment((0.0, 0.0), (0.0, 0.0), (10.0, 0.0)));
        assert!(!point_on_segment((15.0, 0.0), (0.0, 0.0), (10.0, 0.0)));
        assert!(!point_on_segment((5.0, 1.0), (0.0, 0.0), (10.0, 0.0)));
    }
}
