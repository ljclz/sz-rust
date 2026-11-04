#!/bin/bash
# config-defaults.sh — 通用 Soak 工具默认值定义
#
# 本文件是唯一允许出现 sz-rust 硬编码字面量的位置（默认值定义处）。
# 其他脚本禁止硬编码项目专有信息，必须通过参数或 DEFAULT_* 变量引用。
#
# 其他项目（如 sz-pay）使用时：
#   1. 创建自己的 config-defaults.sh（覆盖 DEFAULT_* 变量）
#   2. 或通过命令行参数传入项目专有值

export DEFAULT_PROJECT="sz-rust"
export DEFAULT_PROTECTED_PORT=8300
export DEFAULT_PROTECTED_PROCESS="sz-rust-sz300"
export DEFAULT_WORK_DIR="/www/rust/sz-rust-soak"
export DEFAULT_REPORT_DIR="/www/rust/soak-reports"
export DEFAULT_SOAK_PORTS="8401-8405"
export DEFAULT_RESTART_SCRIPT="/www/rust/sz-rust-soak/restart-sz300.sh"
export DEFAULT_CRON_MARKER="# sz-rust-soak"
export DEFAULT_SOAK_RUNNER="/www/rust/soak-toolkit/soak-runner.sh"