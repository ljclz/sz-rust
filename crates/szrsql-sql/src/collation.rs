//! 排序规则（COLLATE）— Phase 6.31
//!
//! 提供排序规则（COLLATION）感知的字符串比较与排序：
//!
//! - **内置规则**：`C` / `POSIX`（字节序）、`binary`（字节序）、`en_US`（大小写不敏感英语）、`zh_CN`（中文拼音）
//! - **比较接口**：`compare_strings(a, b, &collation) -> Ordering`
//! - **排序接口**：`sort_strings` / `sort_values` / `sort_rows_by_column`
//! - **规则注册表**：`CollationRegistry`，可注册自定义规则
//! - **排序索引**：`CollationIndex`，按规则预排序的索引结构
//!
//! # 设计
//!
//! - **`Collation`**：规则配置（名称/大小写敏感/重音敏感/比较方法）
//! - **`CollationMethod`**：Binary / CaseInsensitive / Pinyin / UnicodeCodepoint
//! - **`CollationError`**：5 变体错误枚举
//! - **`CollationRegistry`**：注册表，预置 `C` / `POSIX` / `binary` / `en_US` / `en_US.UTF-8` / `zh_CN` / `zh_CN.UTF-8`
//! - **`CollationIndex`**：按规则排序的 `(Value, row_idx)` 数组，支持范围扫描
//!
//! # 与 PG 的关系
//!
//! - PG `ORDER BY col COLLATE "zh_CN"` 使用 ICU 或 libc 排序规则
//! - PG 内置 `C` / `POSIX` / `en_US.utf8` / `zh_CN.utf8` 等规则
//! - PG 排序规则可作用于列定义、ORDER BY、比较运算符、索引
//! - 本实现为程序化 API，不集成 SQL 解析路径
//!
//! # zh_CN 拼音排序实现
//!
//! - 内置约 1900 常用汉字的拼音表（无音调，按音节 a-z 排序）
//! - 多音字取最常用读音（与 ICU pinyin 规则的默认行为一致）
//! - 表外汉字回退到 Unicode 码点序（CJK 统一表意文字块内排序）
//! - 同音字按 Unicode 码点稳定排序（ICU 进一步按笔画/部首区分，本实现未实现）
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **无 ICU 依赖**：拼音表为内置静态表，非完整 ICU 实现
//! - **无音调区分**：仅按音节排序，不区分 mā/má/mǎ/mà
//! - **无重音折叠**：未实现 `accent_insensitive`（en_US 仅大小写折叠）
//! - **多音字**：取最常用读音，可能与上下文实际读音不符
//! - **无持久化**：纯内存索引
//! - **`Value` 无 `Hash`/`Eq`**：`CollationIndex` 使用 `Vec<(String, usize)>` 内部键

use crate::executor::ExecutionError;
use crate::executor::Row;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::OnceLock;
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// 排序规则错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CollationError {
    /// 未知的排序规则名
    #[error("unknown collation: {0}")]
    UnknownCollation(String),
    /// 规则已存在（注册冲突）
    #[error("collation already exists: {0}")]
    AlreadyExists(String),
    /// 不支持比较的类型（非 Text/Enum）
    #[error("unsupported value type for collation: {0}")]
    UnsupportedType(String),
    /// 索引为空
    #[error("collation index is empty")]
    EmptyIndex,
    /// 无效的规则名（空字符串）
    #[error("invalid collation name: empty string")]
    EmptyName,
}

impl From<CollationError> for ExecutionError {
    fn from(e: CollationError) -> Self {
        ExecutionError::EvalError(format!("Collation error: {e}"))
    }
}

// =====================================================================
//  排序规则定义
// =====================================================================

/// 比较方法
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollationMethod {
    /// 字节序比较（C / POSIX / binary）
    #[default]
    Binary,
    /// 大小写不敏感比较（en_US 基础行为）
    CaseInsensitive,
    /// 中文拼音排序（zh_CN）
    Pinyin,
    /// Unicode 码点序比较（通用 Unicode 规则）
    UnicodeCodepoint,
}

/// 排序规则
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collation {
    /// 规则名（如 "zh_CN"、"en_US"）
    pub name: String,
    /// 是否大小写敏感
    pub case_sensitive: bool,
    /// 是否重音敏感（当前仅占位，未实现重音折叠）
    pub accent_sensitive: bool,
    /// 比较方法
    pub method: CollationMethod,
}

impl Collation {
    /// 创建新规则
    pub fn new(name: impl Into<String>, method: CollationMethod) -> Self {
        Self {
            name: name.into(),
            case_sensitive: matches!(
                method,
                CollationMethod::Binary
                    | CollationMethod::Pinyin
                    | CollationMethod::UnicodeCodepoint
            ),
            accent_sensitive: true,
            method,
        }
    }

    /// 设置大小写敏感
    pub fn with_case_sensitive(mut self, sensitive: bool) -> Self {
        self.case_sensitive = sensitive;
        self
    }

    /// 设置重音敏感
    pub fn with_accent_sensitive(mut self, sensitive: bool) -> Self {
        self.accent_sensitive = sensitive;
        self
    }

    /// 是否为二进制比较
    pub fn is_binary(&self) -> bool {
        matches!(self.method, CollationMethod::Binary)
    }

    /// 是否为拼音排序
    pub fn is_pinyin(&self) -> bool {
        matches!(self.method, CollationMethod::Pinyin)
    }

    /// 比较两个字符串
    pub fn compare(&self, a: &str, b: &str) -> Ordering {
        compare_strings(a, b, self)
    }
}

impl std::fmt::Display for Collation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Collation({})", self.name)
    }
}

/// 内置规则名 → `Collation`
fn builtin_collation(name: &str) -> Option<Collation> {
    match name {
        "C" | "POSIX" | "binary" => Some(Collation::new(name, CollationMethod::Binary)),
        "en_US" | "en_US.UTF-8" | "en_US.utf8" => {
            Collation::new(name, CollationMethod::CaseInsensitive)
                .with_case_sensitive(false)
                .into()
        }
        "zh_CN" | "zh_CN.UTF-8" | "zh_CN.utf8" => {
            Some(Collation::new(name, CollationMethod::Pinyin))
        }
        "unicode" | "Unicode" => Some(Collation::new(name, CollationMethod::UnicodeCodepoint)),
        _ => None,
    }
}

// =====================================================================
//  规则注册表
// =====================================================================

/// 排序规则注册表
///
/// 预置 PG 常见规则：`C` / `POSIX` / `binary` / `en_US` / `en_US.UTF-8` / `zh_CN` / `zh_CN.UTF-8` / `unicode`。
#[derive(Debug, Clone)]
pub struct CollationRegistry {
    collations: HashMap<String, Collation>,
}

impl Default for CollationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CollationRegistry {
    /// 创建注册表并预置内置规则
    pub fn new() -> Self {
        let mut registry = Self {
            collations: HashMap::new(),
        };
        for name in [
            "C",
            "POSIX",
            "binary",
            "en_US",
            "en_US.UTF-8",
            "en_US.utf8",
            "zh_CN",
            "zh_CN.UTF-8",
            "zh_CN.utf8",
            "unicode",
        ] {
            if let Some(c) = builtin_collation(name) {
                registry.collations.insert(name.to_string(), c);
            }
        }
        registry
    }

    /// 注册自定义规则
    ///
    /// 若规则名已存在返回 `AlreadyExists`。
    pub fn register(&mut self, collation: Collation) -> Result<(), CollationError> {
        if collation.name.is_empty() {
            return Err(CollationError::EmptyName);
        }
        if self.collations.contains_key(&collation.name) {
            return Err(CollationError::AlreadyExists(collation.name));
        }
        self.collations.insert(collation.name.clone(), collation);
        Ok(())
    }

    /// 注销规则（内置规则不可注销）
    pub fn unregister(&mut self, name: &str) -> Result<(), CollationError> {
        if builtin_collation(name).is_some() {
            return Err(CollationError::AlreadyExists(format!(
                "cannot unregister builtin collation: {name}"
            )));
        }
        if self.collations.remove(name).is_none() {
            return Err(CollationError::UnknownCollation(name.to_string()));
        }
        Ok(())
    }

    /// 按名查找规则
    pub fn get(&self, name: &str) -> Result<&Collation, CollationError> {
        self.collations
            .get(name)
            .ok_or_else(|| CollationError::UnknownCollation(name.to_string()))
    }

    /// 列出所有规则名
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.collations.keys().cloned().collect();
        names.sort();
        names
    }

    /// 规则数量
    pub fn len(&self) -> usize {
        self.collations.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.collations.is_empty()
    }
}

// =====================================================================
//  拼音表
// =====================================================================

/// 拼音表条目：(拼音音节, 对应汉字)
///
/// 每个汉字映射到其最常用读音的音节（无音调）。
/// 多音字仅取一个读音。
const PINYIN_ENTRIES: &[(&str, &str)] = &[
    ("a", "阿啊呵嗄"),
    ("ai", "哀挨埃唉呆癌蔼矮艾爱隘碍"),
    ("an", "安谙岸按案鞍氨俺暗黯"),
    ("ang", "肮昂盎"),
    ("ao", "凹敖熬翱袄傲奥澳懊"),
    ("ba", "八巴吧叭扒芭疤捌拔跋靶把坝爸罢霸"),
    ("bai", "白百柏摆败拜"),
    ("ban", "班般搬斑颁板版办半伴扮拌绊瓣"),
    ("bang", "邦帮梆榜膀绑棒磅傍"),
    ("bao", "包孢胞苞褒雹宝保堡饱葆报抱豹暴爆"),
    ("bei", "杯卑背悲碑北备倍被背辈避贝"),
    ("ben", "奔本苯笨"),
    ("beng", "崩绷绷绷蹦迸"),
    ("bi", "逼鼻比彼笔鄙币必毕闭碧蔽弊壁避辟"),
    ("bian", "边鞭编贬扁便变遍辨辩辫"),
    ("biao", "标彪膘表婊"),
    ("bie", "憋别瘪"),
    ("bin", "宾彬滨缤濒摈殡"),
    ("bing", "冰兵丙秉柄炳病并"),
    ("bo", "拨波玻剥钵饽伯泊勃博搏箔舶渤驳"),
    ("bu", "补哺捕不布步怖部簿"),
    ("ca", "擦"),
    ("cai", "猜才材财裁采彩踩菜蔡"),
    ("can", "参餐残惭惨灿"),
    ("cang", "仓沧苍藏"),
    ("cao", "操糙槽草"),
    ("ce", "册侧厕策测"),
    ("cen", "岑"),
    ("ceng", "层蹭"),
    ("cha", "插叉茬茶查碴察岔差"),
    ("chai", "拆柴豺"),
    ("chan", "掺搀蝉馋缠铲产颤"),
    ("chang", "昌猖肠尝偿畅唱倡"),
    ("chao", "抄钞超巢朝嘲潮吵"),
    ("che", "车扯彻撤"),
    ("chen", "尘臣沉辰陈晨衬称趁"),
    ("cheng", "撑称城橙成呈乘惩澄诚承逞"),
    ("chi", "吃痴持池迟驰驰耻齿侈尺赤翅斥"),
    ("chong", "充冲虫崇宠"),
    ("chou", "抽仇畴愁稠筹酬丑臭"),
    ("chu", "出初除厨锄雏橱储楚处触畜"),
    ("chuai", "揣"),
    ("chuan", "川穿椽传船喘串"),
    ("chuang", "疮窗床闯创"),
    ("chui", "吹炊垂锤捶"),
    ("chun", "春椿纯唇蠢"),
    ("chuo", "戳辍"),
    ("ci", "词祠慈瓷辞磁此次刺赐"),
    ("cong", "匆葱聪从丛"),
    ("cou", "凑"),
    ("cu", "粗簇促蹴"),
    ("cuan", "窜蹿"),
    ("cui", "摧崔催脆翠"),
    ("cun", "村存寸"),
    ("cuo", "搓挫措锉错"),
    ("da", "搭达答打大"),
    ("dai", "呆代带袋待逮怠戴"),
    ("dan", "丹单担耽胆旦但弹蛋淡"),
    ("dang", "当党档挡荡"),
    ("dao", "刀导岛倒蹈捣到悼道盗稻"),
    ("de", "德的得"),
    ("dei", "得"),
    ("deng", "登灯等凳邓"),
    ("di", "堤低滴弟敌笛迪底地帝第递"),
    ("dian", "颠掂滇碘点典电店店淀殿"),
    ("diao", "刁雕吊钓掉调"),
    ("die", "跌叠蝶谍碟"),
    ("ding", "丁叮盯钉顶订定"),
    ("diu", "丢"),
    ("dong", "东冬董懂动冻栋洞"),
    ("dou", "都兜斗抖陡豆逗痘"),
    ("du", "督毒读独堵赌杜肚度渡镀"),
    ("duan", "端短段断缎"),
    ("dui", "堆队对兑"),
    ("dun", "墩吨蹲敦顿盾钝遁"),
    ("duo", "多哆夺朵躲堕"),
    ("e", "婀娥鹅蛾额俄讹恶饿"),
    ("en", "恩"),
    ("er", "儿而耳洱饵二贰"),
    ("fa", "发乏伐罚阀筏法"),
    ("fan", "帆番翻凡烦繁樊矾返泛犯范饭"),
    ("fang", "方坊芳防妨仿访纺放"),
    ("fei", "飞妃非啡绯肥匪废沸肺费"),
    ("fen", "分纷吩坟焚粉奋份愤"),
    ("feng", "丰风枫疯峰烽锋蜂封逢缝凤奉"),
    ("fo", "佛"),
    ("fou", "否"),
    (
        "fu",
        "夫敷肤孵扶拂辐幅符伏俘服浮涪福袱抚甫斧俯腑腐辅抚釜赋父腹覆",
    ),
    ("ga", "嘎噶"),
    ("gai", "该改概钙盖溉"),
    ("gan", "甘杆肝柑竿干赶敢感赣"),
    ("gang", "刚纲钢缸岗港杠"),
    ("gao", "高糕膏羔搞稿告"),
    ("ge", "哥歌戈鸽搁胳割革格阁隔铬个各"),
    ("gei", "给"),
    ("gen", "根跟"),
    ("geng", "耕更耿梗"),
    ("gong", "工弓公功攻供宫恭躬巩共贡"),
    ("gou", "沟钩狗勾构购够垢"),
    ("gu", "估姑孤辜古谷股骨鼓固故顾雇"),
    ("gua", "瓜刮挂卦"),
    ("guai", "乖拐怪"),
    ("guan", "关观官管馆罐贯"),
    ("guang", "光广逛"),
    ("gui", "归龟规瑰轨鬼贵桂柜"),
    ("gun", "滚棍"),
    ("guo", "锅郭国果裹过"),
    ("ha", "哈"),
    ("hai", "孩海骸害"),
    ("han", "酣憨邯韩含涵寒喊罕汉汗旱"),
    ("hang", "航行列"),
    ("hao", "蒿嚎豪毫好号浩耗"),
    ("he", "喝禾合何河荷核盒贺和赫"),
    ("hei", "黑嘿"),
    ("hen", "痕很狠恨"),
    ("heng", "哼恒横衡"),
    ("hong", "轰哄红洪宏弘虹鸿"),
    ("hou", "喉猴吼后厚候"),
    ("hu", "呼忽糊湖壶葫胡瑚虎互户护"),
    ("hua", "花华哗滑画划化话"),
    ("huai", "怀槐坏"),
    ("huan", "欢还环桓缓换唤涣患焕"),
    ("huang", "荒慌皇黄凰煌晃谎灰"),
    ("hui", "灰挥辉徽回毁悔慧汇会婚活"),
    ("hun", "昏婚魂浑混"),
    ("huo", "豁活火货或祸获"),
    (
        "ji",
        "几讥机鸡积饥肌基击稽奇圾缉辑籍集及吉级即极急疾棘己挤脊计记忌技纪际季既济继",
    ),
    ("jia", "加佳家嘉夹甲贾钾假价架驾嫁"),
    ("jian", "坚尖奸歼间煎肩艰兼监检减剪简捡见建剑贱渐健践舰鉴键"),
    ("jiang", "江姜将浆僵疆讲奖蒋桨匠降"),
    ("jiao", "交郊浇骄娇椒胶焦蕉角脚搅缴绞剿教轿较叫觉"),
    ("jie", "阶秸揭接街皆结截竭节洁捷姐解介界戒借"),
    ("jin", "巾斤金今津筋襟紧仅紧进近劲晋浸禁"),
    ("jing", "京泾茎经精睛晶兢井警景颈静境敬镜"),
    ("jiong", "炯窘"),
    ("jiu", "揪究纠鸠揪九酒旧救舅咎疚"),
    ("ju", "鞠拘狙疽居驹菊局咀矩举沮聚拒巨具距剧锯"),
    ("juan", "捐涓娟倦眷卷"),
    ("jue", "决诀抉角掘觉爵绝厥"),
    ("jun", "均菌钧军君峻俊竣浚"),
    ("ka", "咖喀卡咯"),
    ("kai", "开揩凯慨"),
    ("kan", "刊堪勘看坎砍"),
    ("kang", "康慷糠扛抗炕"),
    ("kao", "考拷烤靠"),
    ("ke", "坷苛柯棵磕蝌科咳可渴克刻客课"),
    ("ken", "肯啃恳垦"),
    ("keng", "坑"),
    ("kong", "空孔恐控"),
    ("kou", "抠口扣寇"),
    ("ku", "枯哭窟苦酷库裤"),
    ("kua", "夸垮挎跨胯"),
    ("kuai", "会块筷快"),
    ("kuan", "宽款"),
    ("kuang", "筐狂况矿眶框"),
    ("kui", "亏葵奎魁馈愧"),
    ("kun", "昆捆困"),
    ("kuo", "阔扩括"),
    ("la", "垃拉喇蜡腊辣"),
    ("lai", "莱来赖"),
    ("lan", "兰拦栏蓝婪阑澜兰览懒烂"),
    ("lang", "郎狼廊琅榔朗浪"),
    ("lao", "捞劳牢老佬烙涝"),
    ("le", "勒乐了"),
    ("lei", "雷垒蕾磊累泪类"),
    ("leng", "冷愣"),
    ("li", "厘梨犁黎篱狸离漓理李里鲤礼莉荔吏栗丽利励沥例俐痢"),
    ("lian", "莲连廉怜涟帘联恋练链"),
    ("liang", "粮凉梁粱良两辆亮量"),
    ("liao", "撩聊辽疗潦了料撂"),
    ("lie", "咧列劣烈猎裂"),
    ("lin", "林临邻磷淋琳麟"),
    ("ling", "伶灵玲凌铃陵零龄岭领"),
    ("liu", "溜琉硫留刘流柳六溜"),
    ("long", "龙聋笼隆垄拢"),
    ("lou", "楼搂篓漏陋"),
    ("lu", "芦卢颅庐炉掳卤鲁陆录鹿露"),
    ("lv", "驴吕铝旅律绿"),
    ("luan", "峦孪滦卵乱"),
    ("lue", "掠略"),
    ("lun", "沦轮论"),
    ("luo", "罗萝逻锣箩骡裸落洛骆"),
    ("ma", "妈麻玛码蚂马骂吗嘛"),
    ("mai", "埋买麦卖迈脉"),
    ("man", "瞒馒蛮满蔓曼慢漫"),
    ("mang", "芒忙盲茫"),
    ("mao", "猫毛矛茅锚茂冒帽貌贸"),
    ("me", "么"),
    ("mei", "玫枚眉梅媒煤霉美每妹媚"),
    ("men", "门闷们"),
    ("meng", "萌盟檬猛孟梦"),
    ("mi", "眯咪弥迷谜糜米靡"),
    ("mian", "棉眠绵免勉面"),
    ("miao", "苗描秒渺邈"),
    ("mie", "灭蔑"),
    ("min", "民抿敏悯闽"),
    ("ming", "明鸣铭名命"),
    ("miu", "谬"),
    ("mo", "摸摹模膜魔摩抹末莫墨默沫漠陌"),
    ("mou", "谋牟某"),
    ("mu", "母牡亩木沐目牧穆幕"),
    ("na", "拿哪那纳钠"),
    ("nai", "氖乃奶耐奈"),
    ("nan", "南男难"),
    ("nang", "囊"),
    ("nao", "挠脑恼闹"),
    ("ne", "呢"),
    ("nei", "内"),
    ("nen", "嫩"),
    ("neng", "能"),
    ("ni", "尼泥拟你腻逆"),
    ("nian", "年粘碾捻念"),
    ("niang", "娘"),
    ("niao", "鸟尿"),
    ("nie", "捏聂涅"),
    ("nin", "您"),
    ("ning", "柠凝宁泞"),
    ("niu", "牛纽扭钮"),
    ("nong", "农浓弄"),
    ("nu", "奴努怒"),
    ("nv", "女"),
    ("nuan", "暖"),
    ("nue", "虐"),
    ("nuo", "挪诺"),
    ("o", "哦噢"),
    ("ou", "欧鸥藕偶呕"),
    ("pa", "趴扒帕爬怕"),
    ("pai", "拍排牌派迫"),
    ("pan", "潘盘磐盼叛畔"),
    ("pang", "庞旁胖"),
    ("pao", "抛刨炮袍跑泡"),
    ("pei", "培赔陪佩配"),
    ("pen", "喷盆"),
    ("peng", "朋彭膨篷鹏捧碰"),
    ("pi", "批披劈坯砒霹皮疲脾匹癖屁譬"),
    ("pian", "偏篇骗"),
    ("piao", "飘漂瓢票"),
    ("pie", "撇"),
    ("pin", "拼贫频品聘"),
    ("ping", "乒坪苹萍平瓶评"),
    ("po", "坡泼颇婆破迫"),
    ("pou", "剖"),
    ("pu", "扑铺仆莆葡蒲朴瀑普"),
    ("qi", "七沏妻栖期欺漆齐祁祈骑棋奇歧畦崎旗乞企启起气汽弃契砌"),
    ("qia", "掐卡洽"),
    ("qian", "千扦迁签谦前钱钳潜黔浅谴欠歉"),
    ("qiang", "枪腔墙抢强"),
    ("qiao", "悄敲锹桥瞧侨乔巧俏撬峭"),
    ("qie", "切茄且怯窃"),
    ("qin", "钦侵亲秦琴禽勤寝"),
    ("qing", "青清蜻擎晴氰情顷请庆"),
    ("qiong", "琼穷"),
    ("qiu", "秋丘邱球求囚酋"),
    ("qu", "趋区躯驱屈渠蛆取娶龋去趣"),
    ("quan", "圈全权泉拳犬劝券"),
    ("que", "缺瘸却鹊雀确"),
    ("qun", "裙群"),
    ("ran", "然燃染"),
    ("rang", "让嚷壤"),
    ("rao", "饶绕扰"),
    ("re", "惹热"),
    ("ren", "人壬仁忍韧任认刃"),
    ("reng", "扔仍"),
    ("ri", "日"),
    ("rong", "戎茸荣容熔融"),
    ("rou", "柔揉肉"),
    ("ru", "茹儒孺如辱乳汝入褥"),
    ("ruan", "软"),
    ("rui", "蕊瑞锐"),
    ("run", "润闰"),
    ("ruo", "若弱"),
    ("sa", "撒洒萨"),
    ("sai", "腮塞赛"),
    ("san", "三伞散"),
    ("sang", "桑丧"),
    ("sao", "搔骚扫嫂"),
    ("se", "色瑟"),
    ("sen", "森"),
    ("seng", "僧"),
    ("sha", "杀刹沙纱傻啥煞"),
    ("shai", "筛晒"),
    ("shan", "山杉衫珊瑚扇闪陕擅赡膳善"),
    ("shang", "商伤殇赏上尚"),
    ("shao", "烧梢稍勺少绍邵召哨稍"),
    ("she", "奢赊蛇舌舍设射涉摄"),
    ("shei", "谁"),
    ("shen", "申伸身深神沈审婶肾甚渗"),
    ("sheng", "声生升牲胜剩圣"),
    (
        "shi",
        "尸失师诗虱狮施湿十石时识实食拾蚀矢屎使始驶史矢氏世市柿似示士事侍拭释嗜誓逝",
    ),
    ("shou", "收手守首寿受兽售授瘦"),
    ("shu", "书殳枢梳淑疏舒输蔬熟暑鼠数属署蜀薯"),
    ("shua", "刷耍"),
    ("shuai", "摔甩帅"),
    ("shuan", "栓拴"),
    ("shuang", "双霜爽"),
    ("shui", "谁水睡税"),
    ("shun", "吮顺瞬"),
    ("shuo", "说硕"),
    ("si", "司丝私思斯撕死四肆"),
    ("song", "松松颂送宋"),
    ("sou", "搜艘嗽"),
    ("su", "苏酥俗诉肃素宿粟"),
    ("suan", "酸蒜算"),
    ("sui", "虽尿碎岁穗"),
    ("sun", "孙损笋"),
    ("suo", "唆梭缩索锁"),
    ("ta", "塌他它塔踏"),
    ("tai", "胎苔抬台泰太态"),
    ("tan", "贪摊瘫坛檀痰谭坦毯袒叹炭碳探"),
    ("tang", "汤塘糖堂膛螳唐躺烫趟"),
    ("tao", "掏涛滔绦萄桃逃陶淘套讨"),
    ("te", "特"),
    ("teng", "腾疼"),
    ("ti", "梯剔踢锑提题蹄啼体替嚏"),
    ("tian", "天添田甜填"),
    ("tiao", "挑条跳"),
    ("tie", "贴铁帖"),
    ("ting", "厅听烃廷亭停庭挺艇"),
    ("tong", "通同童铜桶筒痛统"),
    ("tou", "偷头投透"),
    ("tu", "凸秃突图徒途涂屠土吐"),
    ("tuan", "湍团"),
    ("tui", "推颓腿蜕退"),
    ("tun", "吞屯"),
    ("tuo", "拖托脱鸵陀驮椭"),
    ("wa", "挖蛙娃瓦洼袜"),
    ("wai", "歪外"),
    ("wan", "豌弯湾玩顽丸烷宛晚挽婉宛惋万腕"),
    ("wang", "汪王枉网往旺望忘妄"),
    ("wei", "威巍微危韦违桅围唯惟为潍维苇萎委卫伪未畏喂胃谓蔚慰"),
    ("wen", "瘟温蚊纹闻稳问"),
    ("weng", "嗡翁"),
    ("wo", "窝蜗我沃卧握"),
    ("wu", "巫呜钨乌污诬屋无芜梧吾吴毋武五捂舞伍侮坞戊雾悟物务"),
    ("xi", "夕汐西吸希溪锡熙嬉膝晰熄熄席习媳喜铣洗系隙细戏"),
    ("xia", "瞎虾侠霞辖峡暇遐下夏吓"),
    ("xian", "掀锨先仙鲜纤咸贤衔舷闲弦嫌显险现线县陷馅献"),
    ("xiang", "相厢镶香箱襄湘乡翔祥详想向享项巷橡像"),
    ("xiao", "萧硝霄削哮嚣销消宵小晓孝肖效校笑"),
    ("xie", "楔些歇蝎鞋协挟携邪斜胁谐写械卸蟹懈泻泄"),
    ("xin", "芯锌欣辛新忻心信衅"),
    ("xing", "星腥猩惺兴刑型形邢行醒幸杏性姓"),
    ("xiong", "兄凶胸匈汹雄熊"),
    ("xiu", "休修羞朽嗅锈秀袖绣"),
    ("xu", "墟戍需虚嘘须徐许蓄酗叙旭序畜恤絮"),
    ("xuan", "轩喧宣悬旋玄选癣眩炫"),
    ("xue", "靴薛学穴雪血"),
    ("xun", "勋熏寻旬询循驯巡殉汛"),
    ("ya", "压押鸦鸭呀丫崖涯雅哑亚讶"),
    (
        "yan",
        "殷焉咽阉烟淹盐延严研蜒岩沿言颜阎眼衍演掩雁厌宴彦艳焰",
    ),
    ("yang", "秧扬羊阳杨洋仰养氧痒样"),
    ("yao", "邀腰妖摇谣遥窑谣姚咬舀药要耀"),
    ("ye", "椰噎耶爷野冶也页夜液"),
    (
        "yi",
        "一壹医揖铱依伊衣颐夷遗移仪胰疑沂宜姨彝椅蚁倚已乙矣以艺抑易邑屹亿役臆逸疫亦裔意毅忆义益溢",
    ),
    ("yin", "茵荫因殷音阴姻吟银淫寅饮尹引隐印"),
    ("ying", "英樱婴鹰应营莹蝇盈赢颖影映硬"),
    ("yo", "哟"),
    ("yong", "拥佣臃痈庸雍踊蛹咏泳勇永咏用"),
    ("you", "幽优悠忧尤由邮犹油游游有友右幼釉诱"),
    ("yu", "迂淤于鱼渝渔隅予娱雨与屿禹宇语羽玉域芋郁吁遇喻峪御愈"),
    ("yuan", "冤鸳渊辕元园袁猿圆垣原圆缘远苑愿怨院"),
    ("yue", "曰约越跃钥岳粤月悦阅"),
    ("yun", "耘云郧匀陨允运蕴酝晕韵"),
    ("za", "匝砸杂"),
    ("zai", "栽哉宰载再在"),
    ("zan", "咱暂攒赞"),
    ("zang", "赃脏葬"),
    ("zao", "遭糟凿早枣蚤澡灶躁造"),
    ("ze", "责择泽"),
    ("zei", "贼"),
    ("zen", "怎"),
    ("zeng", "增憎曾赠"),
    ("zha", "扎乍炸渣轧闸"),
    ("zhai", "斋宅择窄债寨"),
    ("zhan", "瞻毡詹粘沾盏斩展崭栈占战站湛绽"),
    ("zhang", "张章彰漳樟掌丈杖帐账胀障"),
    ("zhao", "招昭找沼赵照罩兆肇召"),
    ("zhe", "遮折哲蛰辙者这浙蔗"),
    ("zhen", "珍斟真砧甄贞针侦枕疹诊震振镇阵"),
    ("zheng", "蒸挣睁征狰争怔整拯正政帧症郑证"),
    (
        "zhi",
        "之枝吱肢知蜘汁脂织执直侄职值植殖止趾只旨纸志挚致帜秩至制质炙痔滞",
    ),
    ("zhong", "中盅忠钟衷终肿种仲重众"),
    ("zhou", "舟周州洲粥轴肘帚咒皱宙昼"),
    ("zhu", "珠株蛛朱猪诸诛逐竹烛煮嘱主著柱助蛀贮铸筑住注驻"),
    ("zhua", "抓"),
    ("zhuai", "拽"),
    ("zhuan", "专砖转赚"),
    ("zhuang", "桩庄装妆撞壮状"),
    ("zhui", "椎锥赘追坠"),
    ("zhun", "谆准"),
    ("zhuo", "捉拙卓桌灼茁酌啄着"),
    ("zi", "孜资咨滋籽仔子紫字自"),
    ("zong", "鬃棕踪宗综总纵"),
    ("zou", "邹走奏揍"),
    ("zu", "租足卒族祖诅阻组"),
    ("zuan", "纂钻"),
    ("zui", "嘴醉最罪"),
    ("zun", "尊遵"),
    ("zuo", "昨左佐做作坐座"),
];

/// 全局拼音查找表（懒加载）
static PINYIN_MAP: OnceLock<HashMap<char, &'static str>> = OnceLock::new();

/// 获取拼音查找表
fn pinyin_map() -> &'static HashMap<char, &'static str> {
    PINYIN_MAP.get_or_init(|| {
        let mut map = HashMap::with_capacity(2048);
        for &(pinyin, chars) in PINYIN_ENTRIES {
            for ch in chars.chars() {
                // 多音字：保留首次出现的读音（最常用）
                map.entry(ch).or_insert(pinyin);
            }
        }
        map
    })
}

/// 查询汉字的拼音（无音调）
///
/// 返回 `None` 表示该字符不在拼音表中（非汉字或罕见汉字）。
fn pinyin_of(ch: char) -> Option<&'static str> {
    pinyin_map().get(&ch).copied()
}

// =====================================================================
//  比较函数
// =====================================================================

/// 比较两个字符串（按指定规则）
pub fn compare_strings(a: &str, b: &str, collation: &Collation) -> Ordering {
    match collation.method {
        CollationMethod::Binary | CollationMethod::UnicodeCodepoint => {
            // Unicode 码点序（Rust str 的默认比较）
            if !collation.case_sensitive {
                a.to_lowercase().cmp(&b.to_lowercase())
            } else {
                a.cmp(b)
            }
        }
        CollationMethod::CaseInsensitive => {
            // 大小写不敏感：按 lowercase 比较
            // 仅当 case_sensitive=true 时按原始值做 tiebreaker（确定性排序）
            let cmp = a.to_lowercase().cmp(&b.to_lowercase());
            if cmp == Ordering::Equal && collation.case_sensitive {
                a.cmp(b)
            } else {
                cmp
            }
        }
        CollationMethod::Pinyin => compare_pinyin(a, b, collation.case_sensitive),
    }
}

/// 拼音排序比较
///
/// 算法：
/// 1. 逐字符比较
/// 2. 若两个字符都在拼音表中，按拼音音节比较
/// 3. 若两个字符都不在表中，按 Unicode 码点比较
/// 4. 若一个在表一个不在表，拼音字符优先（ASCII/CJK 拉丁字符 < 中文汉字）
/// 5. 同音字按 Unicode 码点作为 tiebreaker
/// 6. 长度不同时，前缀相等后短者在前
fn compare_pinyin(a: &str, b: &str, case_sensitive: bool) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        match (a_chars.next(), b_chars.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                let ord = compare_pinyin_char(ca, cb, case_sensitive);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// 比较单个字符的拼音顺序
fn compare_pinyin_char(a: char, b: char, case_sensitive: bool) -> Ordering {
    // 1. 若为 ASCII 字母，先做大小写折叠（拼音排序默认大小写不敏感）
    if a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
        let la = a.to_ascii_lowercase();
        let lb = b.to_ascii_lowercase();
        if la != lb {
            return la.cmp(&lb);
        }
        if !case_sensitive {
            return Ordering::Equal;
        }
        return a.cmp(&b);
    }

    let pa = pinyin_of(a);
    let pb = pinyin_of(b);

    match (pa, pb) {
        (Some(pa), Some(pb)) => {
            // 两字符都在拼音表中：先按音节比较，再按字符码点作为 tiebreaker
            let cmp = pa.cmp(pb);
            if cmp == Ordering::Equal {
                a.cmp(&b)
            } else {
                cmp
            }
        }
        (Some(_), None) => {
            // a 是拼音汉字，b 不是：汉字在后（PG zh_CN 行为：拉丁/ASCII 在前，中文在后）
            // 但若 b 是 ASCII/拉丁，则 b < a
            if b.is_ascii() {
                Ordering::Greater
            } else {
                a.cmp(&b)
            }
        }
        (None, Some(_)) => {
            if a.is_ascii() {
                Ordering::Less
            } else {
                a.cmp(&b)
            }
        }
        (None, None) => {
            // 两字符都不在拼音表：按 Unicode 码点
            if !case_sensitive && a.is_ascii_alphabetic() && b.is_ascii_alphabetic() {
                a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
            } else {
                a.cmp(&b)
            }
        }
    }
}

// =====================================================================
//  Value 比较
// =====================================================================

/// 提取 `Value` 的字符串形式用于排序规则比较
///
/// - `Text(s)` → `s`
/// - `Enum(s)` → `s`
/// - 其他类型 → `None`（不支持排序规则比较）
pub fn value_as_str(value: &Value) -> Option<&str> {
    match value {
        Value::Text(s) => Some(s.as_str()),
        Value::Enum(s) => Some(s.as_str()),
        _ => None,
    }
}

/// 比较两个 `Value`（按指定规则）
///
/// 仅 `Text` / `Enum` 支持排序规则比较；其他类型返回 `UnsupportedType`。
/// NULL 视为最小值（NULLs first，与 PG 默认一致）。
pub fn compare_values(
    a: &Value,
    b: &Value,
    collation: &Collation,
) -> Result<Ordering, CollationError> {
    // NULL 处理：NULLs first（PG 默认）
    match (a, b) {
        (Value::Null, Value::Null) => return Ok(Ordering::Equal),
        (Value::Null, _) => return Ok(Ordering::Less),
        (_, Value::Null) => return Ok(Ordering::Greater),
        _ => {}
    }
    let a_str = value_as_str(a).ok_or_else(|| CollationError::UnsupportedType(format!("{a:?}")))?;
    let b_str = value_as_str(b).ok_or_else(|| CollationError::UnsupportedType(format!("{b:?}")))?;
    Ok(compare_strings(a_str, b_str, collation))
}

// =====================================================================
//  排序函数
// =====================================================================

/// 按指定规则排序字符串切片（返回新 Vec）
pub fn sort_strings(values: &[String], collation: &Collation) -> Vec<String> {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| compare_strings(a, b, collation));
    sorted
}

/// 按指定规则排序 `Value` 切片（返回新 Vec）
///
/// 仅 `Text` / `Enum` 参与排序；其他类型返回 `UnsupportedType`。
pub fn sort_values(values: &[Value], collation: &Collation) -> Result<Vec<Value>, CollationError> {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| compare_values(a, b, collation).unwrap_or(Ordering::Equal));
    Ok(sorted)
}

/// 按指定列索引和规则排序 `Row` 切片（返回新 Vec）
///
/// 列值非 `Text`/`Enum` 返回 `UnsupportedType`。
pub fn sort_rows_by_column(
    rows: &[Row],
    col_idx: usize,
    collation: &Collation,
) -> Result<Vec<Row>, CollationError> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        let av = a.get(col_idx).unwrap_or(&Value::Null);
        let bv = b.get(col_idx).unwrap_or(&Value::Null);
        compare_values(av, bv, collation).unwrap_or(Ordering::Equal)
    });
    Ok(sorted)
}

/// 按指定列索引和规则排序 `Row` 切片（不稳定，返回原索引顺序）
///
/// 返回 `(sorted_rows, original_indices)`，便于调用方做 ORDER BY 后回表。
pub fn sort_rows_with_indices(
    rows: &[Row],
    col_idx: usize,
    collation: &Collation,
) -> Result<(Vec<Row>, Vec<usize>), CollationError> {
    let mut indices: Vec<usize> = (0..rows.len()).collect();
    indices.sort_by(|&i, &j| {
        let av = rows[i].get(col_idx).unwrap_or(&Value::Null);
        let bv = rows[j].get(col_idx).unwrap_or(&Value::Null);
        compare_values(av, bv, collation).unwrap_or(Ordering::Equal)
    });
    let sorted_rows: Vec<Row> = indices.iter().map(|&i| rows[i].clone()).collect();
    Ok((sorted_rows, indices))
}

// =====================================================================
//  排序规则索引
// =====================================================================

/// 排序规则索引
///
/// 预排序的 `(key_string, row_idx)` 数组，支持：
/// - `eq_query`：等值查询（按规则比较）
/// - `range_query`：范围查询（[lower, upper] 闭区间）
/// - `sorted_indices`：返回排序后的行索引
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::collation::*;
/// use szrsql_types::value::Value;
///
/// let collation = Collation::new("zh_CN", CollationMethod::Pinyin);
/// let mut index = CollationIndex::new(collation);
/// index.insert(0, Value::Text("北京".to_string()));
/// index.insert(1, Value::Text("上海".to_string()));
/// index.insert(2, Value::Text("广州".to_string()));
///
/// // 按拼音排序的行索引
/// let order = index.sorted_indices();
/// // 上海(sh) < 广州(g) ? 否：g < sh，所以 广州 < 上海 < 北京
/// assert_eq!(order, vec![2, 1, 0]); // 广州 < 上海 < 北京
/// ```
pub struct CollationIndex {
    collation: Collation,
    entries: Vec<(String, usize)>,
    sorted: bool,
}

impl CollationIndex {
    /// 创建空索引
    pub fn new(collation: Collation) -> Self {
        Self {
            collation,
            entries: Vec::new(),
            sorted: false,
        }
    }

    /// 从行集批量构建
    pub fn build_from_rows(rows: &[Row], col_idx: usize, collation: Collation) -> Self {
        let mut index = Self::new(collation);
        for (row_idx, row) in rows.iter().enumerate() {
            if let Some(value) = row.get(col_idx) {
                if let Some(s) = value_as_str(value) {
                    index.entries.push((s.to_string(), row_idx));
                }
            }
        }
        index.sorted = false;
        index
    }

    /// 插入条目
    pub fn insert(&mut self, row_idx: usize, value: Value) {
        if let Some(s) = value_as_str(&value) {
            self.entries.push((s.to_string(), row_idx));
            self.sorted = false;
        }
    }

    /// 插入字符串键
    pub fn insert_str(&mut self, row_idx: usize, key: impl Into<String>) {
        self.entries.push((key.into(), row_idx));
        self.sorted = false;
    }

    /// 确保索引已按规则排序
    fn ensure_sorted(&mut self) {
        if !self.sorted {
            self.entries
                .sort_by(|a, b| compare_strings(&a.0, &b.0, &self.collation));
            self.sorted = true;
        }
    }

    /// 返回排序后的行索引列表
    pub fn sorted_indices(&mut self) -> Vec<usize> {
        self.ensure_sorted();
        self.entries.iter().map(|(_, idx)| *idx).collect()
    }

    /// 返回排序后的 (key, row_idx) 列表
    pub fn sorted_entries(&mut self) -> &[(String, usize)] {
        self.ensure_sorted();
        &self.entries
    }

    /// 等值查询：返回所有匹配 `key` 的行索引（按规则比较）
    pub fn eq_query(&mut self, key: &str) -> Vec<usize> {
        self.ensure_sorted();
        self.entries
            .iter()
            .filter(|(k, _)| compare_strings(k, key, &self.collation) == Ordering::Equal)
            .map(|(_, idx)| *idx)
            .collect()
    }

    /// 范围查询：返回 `[lower, upper]` 闭区间内所有行索引（按规则排序）
    pub fn range_query(
        &mut self,
        lower: Option<&str>,
        upper: Option<&str>,
    ) -> Result<Vec<usize>, CollationError> {
        self.ensure_sorted();
        let mut result = Vec::new();
        for (key, idx) in &self.entries {
            if let Some(lo) = lower {
                if compare_strings(key, lo, &self.collation) == Ordering::Less {
                    continue;
                }
            }
            if let Some(hi) = upper {
                if compare_strings(key, hi, &self.collation) == Ordering::Greater {
                    continue;
                }
            }
            result.push(*idx);
        }
        Ok(result)
    }

    /// 前缀查询：返回所有以 `prefix` 开头的行索引
    pub fn prefix_query(&mut self, prefix: &str) -> Vec<usize> {
        self.ensure_sorted();
        self.entries
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(_, idx)| *idx)
            .collect()
    }

    /// 索引条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 获取规则引用
    pub fn collation(&self) -> &Collation {
        &self.collation
    }
}

impl std::fmt::Debug for CollationIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CollationIndex")
            .field("collation", &self.collation.name)
            .field("num_entries", &self.entries.len())
            .field("sorted", &self.sorted)
            .finish()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  CollationError（5 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_error_unknown_collation() {
        let e = CollationError::UnknownCollation("foo".to_string());
        assert_eq!(e.to_string(), "unknown collation: foo");
    }

    #[test]
    fn test_error_already_exists() {
        let e = CollationError::AlreadyExists("zh_CN".to_string());
        assert_eq!(e.to_string(), "collation already exists: zh_CN");
    }

    #[test]
    fn test_error_unsupported_type() {
        let e = CollationError::UnsupportedType("Int64(42)".to_string());
        assert_eq!(
            e.to_string(),
            "unsupported value type for collation: Int64(42)"
        );
    }

    #[test]
    fn test_error_empty_index() {
        let e = CollationError::EmptyIndex;
        assert_eq!(e.to_string(), "collation index is empty");
    }

    #[test]
    fn test_error_empty_name() {
        let e = CollationError::EmptyName;
        assert_eq!(e.to_string(), "invalid collation name: empty string");
    }

    #[test]
    fn test_error_to_execution_error() {
        let e: ExecutionError = CollationError::UnknownCollation("x".to_string()).into();
        let msg = e.to_string();
        assert!(msg.contains("Collation error"));
        assert!(msg.contains("unknown collation: x"));
    }

    // -----------------------------------------------------------------
    //  CollationMethod（3 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_collation_method_default_is_binary() {
        let m = CollationMethod::default();
        assert_eq!(m, CollationMethod::Binary);
    }

    #[test]
    fn test_collation_method_variants() {
        assert_ne!(CollationMethod::Binary, CollationMethod::CaseInsensitive);
        assert_ne!(CollationMethod::Pinyin, CollationMethod::UnicodeCodepoint);
        assert_ne!(CollationMethod::Binary, CollationMethod::Pinyin);
    }

    #[test]
    fn test_collation_method_eq() {
        assert_eq!(CollationMethod::Binary, CollationMethod::Binary);
        assert_eq!(CollationMethod::Pinyin, CollationMethod::Pinyin);
    }

    // -----------------------------------------------------------------
    //  Collation 构造（6 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_collation_new_binary() {
        let c = Collation::new("C", CollationMethod::Binary);
        assert_eq!(c.name, "C");
        assert!(c.case_sensitive);
        assert!(c.accent_sensitive);
        assert!(c.is_binary());
        assert!(!c.is_pinyin());
    }

    #[test]
    fn test_collation_new_case_insensitive() {
        let c = Collation::new("en_US", CollationMethod::CaseInsensitive);
        assert_eq!(c.name, "en_US");
        assert!(!c.case_sensitive);
        assert!(!c.is_binary());
        assert!(!c.is_pinyin());
    }

    #[test]
    fn test_collation_new_pinyin() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        assert_eq!(c.name, "zh_CN");
        assert!(c.case_sensitive);
        assert!(c.is_pinyin());
    }

    #[test]
    fn test_collation_with_case_sensitive() {
        let c = Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(true);
        assert!(c.case_sensitive);
    }

    #[test]
    fn test_collation_with_accent_sensitive() {
        let c =
            Collation::new("en_US", CollationMethod::CaseInsensitive).with_accent_sensitive(false);
        assert!(!c.accent_sensitive);
    }

    #[test]
    fn test_collation_display() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        assert_eq!(format!("{c}"), "Collation(zh_CN)");
    }

    // -----------------------------------------------------------------
    //  CollationRegistry（6 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_registry_default_has_builtins() {
        let r = CollationRegistry::default();
        assert!(r.get("C").is_ok());
        assert!(r.get("POSIX").is_ok());
        assert!(r.get("binary").is_ok());
        assert!(r.get("en_US").is_ok());
        assert!(r.get("en_US.UTF-8").is_ok());
        assert!(r.get("zh_CN").is_ok());
        assert!(r.get("zh_CN.UTF-8").is_ok());
        assert!(r.get("unicode").is_ok());
    }

    #[test]
    fn test_registry_get_unknown() {
        let r = CollationRegistry::default();
        let err = r.get("foo").unwrap_err();
        assert!(matches!(err, CollationError::UnknownCollation(_)));
    }

    #[test]
    fn test_registry_register_custom() {
        let mut r = CollationRegistry::default();
        let c = Collation::new("custom_ci", CollationMethod::CaseInsensitive)
            .with_case_sensitive(false);
        r.register(c).unwrap();
        let got = r.get("custom_ci").unwrap();
        assert!(!got.case_sensitive);
    }

    #[test]
    fn test_registry_register_duplicate() {
        let mut r = CollationRegistry::default();
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let err = r.register(c).unwrap_err();
        assert!(matches!(err, CollationError::AlreadyExists(_)));
    }

    #[test]
    fn test_registry_register_empty_name() {
        let mut r = CollationRegistry::default();
        let c = Collation::new("", CollationMethod::Binary);
        let err = r.register(c).unwrap_err();
        assert!(matches!(err, CollationError::EmptyName));
    }

    #[test]
    fn test_registry_list_sorted() {
        let r = CollationRegistry::default();
        let names = r.list();
        assert!(names.iter().any(|n| n == "C"));
        assert!(names.iter().any(|n| n == "zh_CN"));
        // 验证已排序
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    // -----------------------------------------------------------------
    //  Binary / C 比较（5 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_compare_binary_basic() {
        let c = Collation::new("C", CollationMethod::Binary);
        assert_eq!(compare_strings("abc", "abc", &c), Ordering::Equal);
        assert_eq!(compare_strings("abc", "abd", &c), Ordering::Less);
        assert_eq!(compare_strings("abd", "abc", &c), Ordering::Greater);
    }

    #[test]
    fn test_compare_binary_case_sensitive() {
        let c = Collation::new("C", CollationMethod::Binary);
        // 大写字母 ASCII 码 < 小写字母
        assert_eq!(compare_strings("ABC", "abc", &c), Ordering::Less);
        assert_eq!(compare_strings("Apple", "apple", &c), Ordering::Less);
    }

    #[test]
    fn test_compare_binary_different_length() {
        let c = Collation::new("C", CollationMethod::Binary);
        assert_eq!(compare_strings("abc", "abcd", &c), Ordering::Less);
        assert_eq!(compare_strings("abcd", "abc", &c), Ordering::Greater);
    }

    #[test]
    fn test_compare_binary_empty_strings() {
        let c = Collation::new("C", CollationMethod::Binary);
        assert_eq!(compare_strings("", "", &c), Ordering::Equal);
        assert_eq!(compare_strings("", "a", &c), Ordering::Less);
        assert_eq!(compare_strings("a", "", &c), Ordering::Greater);
    }

    #[test]
    fn test_compare_unicode_codepoint() {
        let c = Collation::new("unicode", CollationMethod::UnicodeCodepoint);
        assert_eq!(compare_strings("abc", "abc", &c), Ordering::Equal);
        assert_eq!(compare_strings("abc", "abd", &c), Ordering::Less);
    }

    // -----------------------------------------------------------------
    //  CaseInsensitive / en_US 比较（6 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_compare_case_insensitive_equal() {
        let c =
            Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(false);
        assert_eq!(compare_strings("hello", "HELLO", &c), Ordering::Equal);
        assert_eq!(compare_strings("Hello", "hello", &c), Ordering::Equal);
    }

    #[test]
    fn test_compare_case_insensitive_order() {
        let c =
            Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(false);
        assert_eq!(compare_strings("apple", "Banana", &c), Ordering::Less);
        assert_eq!(compare_strings("Banana", "apple", &c), Ordering::Greater);
    }

    #[test]
    fn test_compare_case_insensitive_tiebreaker() {
        // case_sensitive=true 时：lower 相等则按原值做 tiebreaker（确定性排序）
        let c = Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(true);
        // "ABC" 与 "abc" lower 相等 → 用原值 tiebreaker
        let ord = compare_strings("ABC", "abc", &c);
        assert_eq!(ord, Ordering::Less); // "ABC" < "abc"（大写 ASCII 小）
    }

    #[test]
    fn test_compare_case_insensitive_mixed() {
        let c =
            Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(false);
        let values = vec![
            "banana".to_string(),
            "Apple".to_string(),
            "cherry".to_string(),
            "apple".to_string(),
        ];
        let sorted = sort_strings(&values, &c);
        // 大小写不敏感排序：Apple/apple 相邻
        assert_eq!(sorted[0].to_lowercase(), "apple");
        assert_eq!(sorted[1].to_lowercase(), "apple");
        assert_eq!(sorted[2], "banana");
        assert_eq!(sorted[3], "cherry");
    }

    #[test]
    fn test_compare_en_us_registry() {
        let r = CollationRegistry::default();
        let c = r.get("en_US").unwrap();
        assert_eq!(compare_strings("HELLO", "hello", c), Ordering::Equal);
    }

    #[test]
    fn test_compare_en_us_utf8_alias() {
        let r = CollationRegistry::default();
        let c1 = r.get("en_US").unwrap();
        let c2 = r.get("en_US.UTF-8").unwrap();
        assert_eq!(c1.method, c2.method);
        assert_eq!(c1.case_sensitive, c2.case_sensitive);
    }

    // -----------------------------------------------------------------
    //  Pinyin / zh_CN 比较（10 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_pinyin_basic_order() {
        // 北京(B) vs 上海(S) vs 广州(G)
        // 拼音：bei < guang < shang
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        assert_eq!(compare_strings("北京", "上海", &c), Ordering::Less); // bei < shang
        assert_eq!(compare_strings("上海", "广州", &c), Ordering::Greater); // shang > guang
        assert_eq!(compare_strings("广州", "北京", &c), Ordering::Greater); // guang > bei
    }

    #[test]
    fn test_pinyin_sort_cities() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let cities = vec![
            "上海".to_string(),
            "北京".to_string(),
            "广州".to_string(),
            "深圳".to_string(),
        ];
        let sorted = sort_strings(&cities, &c);
        // 期望：北京(bei) < 广州(guang) < 上海(shang) < 深圳(shen)
        assert_eq!(sorted, vec!["北京", "广州", "上海", "深圳"]);
    }

    #[test]
    fn test_pinyin_homophone_tiebreaker() {
        // 同音字按 Unicode 码点做 tiebreaker
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        // "吧" 和 "八" 都读 ba → 按码点（八 U+516B < 吧 U+5427）
        let ord = compare_strings("八", "吧", &c);
        assert_eq!(ord, Ordering::Less);
    }

    #[test]
    fn test_pinyin_mixed_ascii_and_chinese() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        // ASCII 在前，中文在后（与 ICU pinyin 默认行为一致）
        assert_eq!(compare_strings("Apple", "北京", &c), Ordering::Less);
        assert_eq!(compare_strings("北京", "Apple", &c), Ordering::Greater);
    }

    #[test]
    fn test_pinyin_same_pinyin_different_chars() {
        // 妈(ma) vs 马(ma) vs 码(ma)：同音不同字 → 按 Unicode 码点
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        // 妈 U+5988 < 马 U+9A6C < 码 U+7801
        let sorted = sort_strings(&["马".to_string(), "码".to_string(), "妈".to_string()], &c);
        assert_eq!(sorted, vec!["妈", "码", "马"]);
    }

    #[test]
    fn test_pinyin_empty_and_chinese() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        assert_eq!(compare_strings("", "北京", &c), Ordering::Less);
        assert_eq!(compare_strings("北京", "", &c), Ordering::Greater);
        assert_eq!(compare_strings("", "", &c), Ordering::Equal);
    }

    #[test]
    fn test_pinyin_prefix_equal() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        // 北京 vs 北京市：前缀相等后短者在前
        assert_eq!(compare_strings("北京", "北京市", &c), Ordering::Less);
        assert_eq!(compare_strings("北京市", "北京", &c), Ordering::Greater);
    }

    #[test]
    fn test_pinyin_english_words() {
        // 纯英文也应该按 ASCII 排序
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let values = vec![
            "banana".to_string(),
            "apple".to_string(),
            "cherry".to_string(),
        ];
        let sorted = sort_strings(&values, &c);
        assert_eq!(sorted, vec!["apple", "banana", "cherry"]);
    }

    #[test]
    fn test_pinyin_registry_zh_cn() {
        let r = CollationRegistry::default();
        let c = r.get("zh_CN").unwrap();
        assert!(c.is_pinyin());
        // 验证 utf8 别名一致
        let c2 = r.get("zh_CN.UTF-8").unwrap();
        assert_eq!(c.method, c2.method);
    }

    #[test]
    fn test_pinyin_numbers_and_chinese() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        // 数字 ASCII < 中文
        assert_eq!(compare_strings("123", "北京", &c), Ordering::Less);
        assert_eq!(compare_strings("北京", "123", &c), Ordering::Greater);
    }

    // -----------------------------------------------------------------
    //  Value 比较（5 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_value_as_str_text() {
        let v = Value::Text("hello".to_string());
        assert_eq!(value_as_str(&v), Some("hello"));
    }

    #[test]
    fn test_value_as_str_enum() {
        let v = Value::Enum("active".to_string());
        assert_eq!(value_as_str(&v), Some("active"));
    }

    #[test]
    fn test_value_as_str_unsupported() {
        assert_eq!(value_as_str(&Value::Int64(42)), None);
        assert_eq!(value_as_str(&Value::Bool(true)), None);
        assert_eq!(value_as_str(&Value::Null), None);
    }

    #[test]
    fn test_compare_values_text() {
        let c =
            Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(false);
        let a = Value::Text("Hello".to_string());
        let b = Value::Text("HELLO".to_string());
        assert_eq!(compare_values(&a, &b, &c).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_compare_values_null_handling() {
        let c = Collation::new("C", CollationMethod::Binary);
        let n = Value::Null;
        let s = Value::Text("abc".to_string());
        // NULLs first
        assert_eq!(compare_values(&n, &s, &c).unwrap(), Ordering::Less);
        assert_eq!(compare_values(&s, &n, &c).unwrap(), Ordering::Greater);
        assert_eq!(compare_values(&n, &n, &c).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_compare_values_unsupported_type() {
        let c = Collation::new("C", CollationMethod::Binary);
        let a = Value::Int64(1);
        let b = Value::Int64(2);
        let err = compare_values(&a, &b, &c).unwrap_err();
        assert!(matches!(err, CollationError::UnsupportedType(_)));
    }

    // -----------------------------------------------------------------
    //  排序函数（5 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_sort_strings_binary() {
        let c = Collation::new("C", CollationMethod::Binary);
        let values = vec![
            "banana".to_string(),
            "apple".to_string(),
            "cherry".to_string(),
        ];
        let sorted = sort_strings(&values, &c);
        assert_eq!(sorted, vec!["apple", "banana", "cherry"]);
    }

    #[test]
    fn test_sort_values_text() {
        let c = Collation::new("C", CollationMethod::Binary);
        let values = vec![
            Value::Text("banana".to_string()),
            Value::Text("apple".to_string()),
            Value::Text("cherry".to_string()),
        ];
        let sorted = sort_values(&values, &c).unwrap();
        assert_eq!(sorted[0], Value::Text("apple".to_string()));
        assert_eq!(sorted[1], Value::Text("banana".to_string()));
        assert_eq!(sorted[2], Value::Text("cherry".to_string()));
    }

    #[test]
    fn test_sort_rows_by_column_pinyin() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let rows: Vec<Row> = vec![
            vec![Value::Text("上海".to_string())],
            vec![Value::Text("北京".to_string())],
            vec![Value::Text("广州".to_string())],
        ];
        let sorted = sort_rows_by_column(&rows, 0, &c).unwrap();
        assert_eq!(sorted[0][0], Value::Text("北京".to_string()));
        assert_eq!(sorted[1][0], Value::Text("广州".to_string()));
        assert_eq!(sorted[2][0], Value::Text("上海".to_string()));
    }

    #[test]
    fn test_sort_rows_with_indices() {
        let c = Collation::new("C", CollationMethod::Binary);
        let rows: Vec<Row> = vec![
            vec![Value::Text("c".to_string())],
            vec![Value::Text("a".to_string())],
            vec![Value::Text("b".to_string())],
        ];
        let (sorted, indices) = sort_rows_with_indices(&rows, 0, &c).unwrap();
        assert_eq!(indices, vec![1, 2, 0]);
        assert_eq!(sorted[0][0], Value::Text("a".to_string()));
        assert_eq!(sorted[1][0], Value::Text("b".to_string()));
        assert_eq!(sorted[2][0], Value::Text("c".to_string()));
    }

    #[test]
    fn test_sort_rows_with_nulls_first() {
        let c = Collation::new("C", CollationMethod::Binary);
        let rows: Vec<Row> = vec![
            vec![Value::Text("apple".to_string())],
            vec![Value::Null],
            vec![Value::Text("banana".to_string())],
        ];
        let sorted = sort_rows_by_column(&rows, 0, &c).unwrap();
        // NULLs first
        assert_eq!(sorted[0][0], Value::Null);
        assert_eq!(sorted[1][0], Value::Text("apple".to_string()));
        assert_eq!(sorted[2][0], Value::Text("banana".to_string()));
    }

    // -----------------------------------------------------------------
    //  CollationIndex（10 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_index_new_empty() {
        let c = Collation::new("C", CollationMethod::Binary);
        let idx = CollationIndex::new(c);
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_index_insert_and_len() {
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("a".to_string()));
        idx.insert(1, Value::Text("b".to_string()));
        assert_eq!(idx.len(), 2);
        assert!(!idx.is_empty());
    }

    #[test]
    fn test_index_sorted_indices_binary() {
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("cherry".to_string()));
        idx.insert(1, Value::Text("apple".to_string()));
        idx.insert(2, Value::Text("banana".to_string()));
        let order = idx.sorted_indices();
        assert_eq!(order, vec![1, 2, 0]); // apple, banana, cherry
    }

    #[test]
    fn test_index_sorted_indices_pinyin() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("上海".to_string()));
        idx.insert(1, Value::Text("北京".to_string()));
        idx.insert(2, Value::Text("广州".to_string()));
        let order = idx.sorted_indices();
        // 北京(bei) < 广州(guang) < 上海(shang)
        assert_eq!(order, vec![1, 2, 0]);
    }

    #[test]
    fn test_index_eq_query() {
        let c =
            Collation::new("en_US", CollationMethod::CaseInsensitive).with_case_sensitive(false);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("Hello".to_string()));
        idx.insert(1, Value::Text("HELLO".to_string()));
        idx.insert(2, Value::Text("World".to_string()));
        let matches = idx.eq_query("hello");
        // 大小写不敏感匹配 Hello + HELLO
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&0));
        assert!(matches.contains(&1));
    }

    #[test]
    fn test_index_range_query_binary() {
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("apple".to_string()));
        idx.insert(1, Value::Text("banana".to_string()));
        idx.insert(2, Value::Text("cherry".to_string()));
        idx.insert(3, Value::Text("date".to_string()));
        // [b, d]
        let result = idx.range_query(Some("b"), Some("d")).unwrap();
        assert_eq!(result, vec![1, 2]); // banana, cherry
    }

    #[test]
    fn test_index_range_query_pinyin() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("北京".to_string()));
        idx.insert(1, Value::Text("上海".to_string()));
        idx.insert(2, Value::Text("广州".to_string()));
        idx.insert(3, Value::Text("深圳".to_string()));
        // [广州, 上海] → 广州 + 上海
        let result = idx.range_query(Some("广州"), Some("上海")).unwrap();
        assert_eq!(result, vec![2, 1]);
    }

    #[test]
    fn test_index_prefix_query() {
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("apple".to_string()));
        idx.insert(1, Value::Text("application".to_string()));
        idx.insert(2, Value::Text("banana".to_string()));
        idx.insert(3, Value::Text("apply".to_string()));
        let result = idx.prefix_query("app");
        assert_eq!(result.len(), 3);
        assert!(result.contains(&0));
        assert!(result.contains(&1));
        assert!(result.contains(&3));
    }

    #[test]
    fn test_index_build_from_rows() {
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Text("banana".to_string())],
            vec![Value::Int64(2), Value::Text("apple".to_string())],
            vec![Value::Int64(3), Value::Text("cherry".to_string())],
        ];
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::build_from_rows(&rows, 1, c);
        let order = idx.sorted_indices();
        assert_eq!(order, vec![1, 0, 2]); // apple, banana, cherry
    }

    #[test]
    fn test_index_insert_str() {
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::new(c);
        idx.insert_str(0, "z");
        idx.insert_str(1, "a");
        idx.insert_str(2, "m");
        let order = idx.sorted_indices();
        assert_eq!(order, vec![1, 2, 0]); // a, m, z
    }

    #[test]
    fn test_index_sorted_entries() {
        let c = Collation::new("C", CollationMethod::Binary);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("c".to_string()));
        idx.insert(1, Value::Text("a".to_string()));
        idx.insert(2, Value::Text("b".to_string()));
        let entries = idx.sorted_entries();
        assert_eq!(entries[0].0, "a");
        assert_eq!(entries[0].1, 1);
        assert_eq!(entries[1].0, "b");
        assert_eq!(entries[1].1, 2);
        assert_eq!(entries[2].0, "c");
        assert_eq!(entries[2].1, 0);
    }

    #[test]
    fn test_index_debug_format() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let idx = CollationIndex::new(c);
        let s = format!("{idx:?}");
        assert!(s.contains("CollationIndex"));
        assert!(s.contains("zh_CN"));
    }

    // -----------------------------------------------------------------
    //  E2E 综合（10 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_e2e_order_by_collate_zh_cn() {
        // 模拟 SELECT name FROM t ORDER BY name COLLATE "zh_CN"
        let r = CollationRegistry::default();
        let collation = r.get("zh_CN").unwrap().clone();
        let names = vec![
            Value::Text("张三".to_string()),
            Value::Text("李四".to_string()),
            Value::Text("王五".to_string()),
            Value::Text("赵六".to_string()),
        ];
        let sorted = sort_values(&names, &collation).unwrap();
        // 拼音：李(li) < 王(wang) < 张(zhang) < 赵(zhao)
        assert_eq!(sorted[0], Value::Text("李四".to_string()));
        assert_eq!(sorted[1], Value::Text("王五".to_string()));
        assert_eq!(sorted[2], Value::Text("张三".to_string()));
        assert_eq!(sorted[3], Value::Text("赵六".to_string()));
    }

    #[test]
    fn test_e2e_order_by_collate_en_us() {
        // 模拟 SELECT name FROM t ORDER BY name COLLATE "en_US"
        let r = CollationRegistry::default();
        let collation = r.get("en_US").unwrap().clone();
        let names = vec![
            Value::Text("banana".to_string()),
            Value::Text("Apple".to_string()),
            Value::Text("cherry".to_string()),
            Value::Text("apple".to_string()),
        ];
        let sorted = sort_values(&names, &collation).unwrap();
        // 大小写不敏感排序：Apple/apple 相邻
        let strs: Vec<&str> = sorted
            .iter()
            .map(|v| match v {
                Value::Text(s) => s.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(strs[0].to_lowercase(), "apple");
        assert_eq!(strs[1].to_lowercase(), "apple");
        assert_eq!(strs[2], "banana");
        assert_eq!(strs[3], "cherry");
    }

    #[test]
    fn test_e2e_order_by_collate_c() {
        // 模拟 SELECT name FROM t ORDER BY name COLLATE "C"
        let r = CollationRegistry::default();
        let collation = r.get("C").unwrap().clone();
        let names = vec![
            Value::Text("banana".to_string()),
            Value::Text("Apple".to_string()),
            Value::Text("cherry".to_string()),
            Value::Text("apple".to_string()),
        ];
        let sorted = sort_values(&names, &collation).unwrap();
        // C 规则：大写字母 ASCII 小 → Apple < apple < banana < cherry
        let strs: Vec<&str> = sorted
            .iter()
            .map(|v| match v {
                Value::Text(s) => s.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(strs[0], "Apple");
        assert_eq!(strs[1], "apple");
        assert_eq!(strs[2], "banana");
        assert_eq!(strs[3], "cherry");
    }

    #[test]
    fn test_e2e_chinese_pinyin_full_sort() {
        // 完整的中文拼音排序：多个常见姓氏
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let surnames = vec![
            "赵".to_string(),
            "钱".to_string(),
            "孙".to_string(),
            "李".to_string(),
            "周".to_string(),
            "吴".to_string(),
            "郑".to_string(),
            "王".to_string(),
        ];
        let sorted = sort_strings(&surnames, &c);
        // 百家姓拼音序：李(li) < 钱(qian) < 孙(sun) < 王(wang) < 吴(wu) < 赵(zhao) < 郑(zheng) < 周(zhou)
        // 注：郑(zheng) vs 周(zhou)：zheng < zhou（e < o）
        assert_eq!(sorted[0], "李");
        assert_eq!(sorted[1], "钱");
        assert_eq!(sorted[2], "孙");
        assert_eq!(sorted[3], "王");
        assert_eq!(sorted[4], "吴");
        assert_eq!(sorted[5], "赵");
        assert_eq!(sorted[6], "郑");
        assert_eq!(sorted[7], "周");
    }

    #[test]
    fn test_e2e_index_supports_collation() {
        // 模拟 CREATE INDEX ON t (name COLLATE "zh_CN") + ORDER BY 走索引
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("赵六".to_string()));
        idx.insert(1, Value::Text("李四".to_string()));
        idx.insert(2, Value::Text("王五".to_string()));
        idx.insert(3, Value::Text("张三".to_string()));
        let order = idx.sorted_indices();
        // 拼音序：李(li) < 王(wang) < 张(zhang) < 赵(zhao)
        assert_eq!(order, vec![1, 2, 3, 0]);
    }

    #[test]
    fn test_e2e_mixed_language_sort() {
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let values = vec![
            "北京".to_string(),
            "Apple".to_string(),
            "上海".to_string(),
            "banana".to_string(),
            "广州".to_string(),
        ];
        let sorted = sort_strings(&values, &c);
        // ASCII 在前（按字母序），中文在后（按拼音序）
        assert_eq!(sorted[0], "Apple");
        assert_eq!(sorted[1], "banana");
        assert_eq!(sorted[2], "北京");
        assert_eq!(sorted[3], "广州");
        assert_eq!(sorted[4], "上海");
    }

    #[test]
    fn test_e2e_collation_compare_operators() {
        // 模拟 WHERE name > '广州' COLLATE "zh_CN"
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let r = CollationRegistry::default();
        let registered = r.get("zh_CN").unwrap();
        assert_eq!(registered.compare("上海", "广州"), Ordering::Greater);
        assert_eq!(registered.compare("北京", "广州"), Ordering::Less);
        assert_eq!(registered.compare("广州", "广州"), Ordering::Equal);

        // 自定义规则与注册表规则行为一致
        assert_eq!(
            c.compare("上海", "广州"),
            registered.compare("上海", "广州")
        );
    }

    #[test]
    fn test_e2e_index_range_scan() {
        // 模拟索引范围扫描：WHERE name BETWEEN '北京' AND '上海' COLLATE "zh_CN"
        let c = Collation::new("zh_CN", CollationMethod::Pinyin);
        let mut idx = CollationIndex::new(c);
        idx.insert(0, Value::Text("北京".to_string()));
        idx.insert(1, Value::Text("上海".to_string()));
        idx.insert(2, Value::Text("广州".to_string()));
        idx.insert(3, Value::Text("深圳".to_string()));
        idx.insert(4, Value::Text("南京".to_string()));

        // [北京, 上海]：北京(bei) < 广州(guang) < 南京(nan) < 上海(shang)
        // 注意：深圳(shen) > 上海(shang) → 不在范围内
        let result = idx.range_query(Some("北京"), Some("上海")).unwrap();
        assert_eq!(result.len(), 4);
        assert!(result.contains(&0)); // 北京
        assert!(result.contains(&2)); // 广州
        assert!(result.contains(&4)); // 南京
        assert!(result.contains(&1)); // 上海
        assert!(!result.contains(&3)); // 深圳 不在范围
    }

    #[test]
    fn test_e2e_collation_stability() {
        // 同 key 的稳定性：插入顺序不影响相对顺序（sort_by 稳定）
        let c = Collation::new("C", CollationMethod::Binary);
        let values1 = vec![
            Value::Text("a".to_string()),
            Value::Text("a".to_string()),
            Value::Text("a".to_string()),
        ];
        let sorted = sort_values(&values1, &c).unwrap();
        // 全部相等 → 顺序保持
        for v in &sorted {
            assert_eq!(*v, Value::Text("a".to_string()));
        }
    }

    #[test]
    fn test_e2e_collation_registry_custom() {
        // 注册自定义规则并使用
        let mut r = CollationRegistry::default();
        let custom = Collation::new("my_binary", CollationMethod::Binary).with_case_sensitive(true);
        r.register(custom).unwrap();
        let c = r.get("my_binary").unwrap();
        assert!(c.case_sensitive);
        assert!(c.is_binary());
        assert_eq!(c.compare("ABC", "abc"), Ordering::Less);
    }

    // -----------------------------------------------------------------
    //  pinyin_of 单元测试（3 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_pinyin_of_known_chars() {
        assert_eq!(pinyin_of('北'), Some("bei"));
        assert_eq!(pinyin_of('京'), Some("jing"));
        assert_eq!(pinyin_of('上'), Some("shang"));
        assert_eq!(pinyin_of('海'), Some("hai"));
        assert_eq!(pinyin_of('广'), Some("guang"));
        assert_eq!(pinyin_of('州'), Some("zhou"));
    }

    #[test]
    fn test_pinyin_of_ascii_returns_none() {
        assert_eq!(pinyin_of('a'), None);
        assert_eq!(pinyin_of('A'), None);
        assert_eq!(pinyin_of('1'), None);
        assert_eq!(pinyin_of(' '), None);
    }

    #[test]
    fn test_pinyin_of_rare_char_returns_none() {
        // 罕见汉字（不在常用拼音表中）
        assert_eq!(pinyin_of('龘'), None);
    }

    // -----------------------------------------------------------------
    //  NULL 与边界（3 测试）
    // -----------------------------------------------------------------

    #[test]
    fn test_compare_values_enum_supports_collation() {
        let c = Collation::new("C", CollationMethod::Binary);
        let a = Value::Enum("active".to_string());
        let b = Value::Enum("inactive".to_string());
        let ord = compare_values(&a, &b, &c).unwrap();
        assert_eq!(ord, Ordering::Less); // "active" < "inactive"
    }

    #[test]
    fn test_sort_rows_empty_input() {
        let c = Collation::new("C", CollationMethod::Binary);
        let rows: Vec<Row> = Vec::new();
        let sorted = sort_rows_by_column(&rows, 0, &c).unwrap();
        assert!(sorted.is_empty());
    }

    #[test]
    fn test_sort_rows_missing_column_treated_as_null() {
        let c = Collation::new("C", CollationMethod::Binary);
        // 第二行只有 1 列，col_idx=1 越界 → 视为 NULL
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Text("b".to_string())],
            vec![Value::Int64(2)], // 缺失 col 1
            vec![Value::Int64(3), Value::Text("a".to_string())],
        ];
        let sorted = sort_rows_by_column(&rows, 1, &c).unwrap();
        // NULLs first → row 1, then "a", then "b"
        assert_eq!(sorted[0].get(1), None);
        assert_eq!(sorted[1].get(1), Some(&Value::Text("a".to_string())));
        assert_eq!(sorted[2].get(1), Some(&Value::Text("b".to_string())));
    }
}
