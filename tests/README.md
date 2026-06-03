# 集成测试设计文档

本文档描述 sugar-wifi-conf-rs 项目的集成测试策略。

## 测试目标

1. **验证配置解析**：确保 `custom_config.json` 正确解析
2. **验证协议层**：BLE 数据包重组、分帧逻辑正确
3. **验证业务逻辑**：WiFi 请求解析、自定义命令执行
4. **验证模块集成**：各模块协同工作

## 目录结构

```
tests/
├── README.md           # 本文件
├── fixtures/           # 测试数据文件
│   ├── config/         # 配置 JSON 文件
│   └── wpa_supplicant/ # wpa_supplicant.conf 示例
├── helpers/            # 测试辅助代码
│   ├── mod.rs
│   ├── mock_bluez.rs   # Mock BlueZ D-Bus 接口
│   └── temp_dir.rs     # 临时目录管理
├── test_config.rs      # 配置模块集成测试
├── test_proto.rs       # 协议层集成测试
├── test_wifi.rs        # WiFi 模块集成测试
├── test_custom_command.rs  # 自定义命令测试
└── test_gatt.rs        # GATT 模块测试

# 构建和运行
cargo test --test test_config
cargo test --test test_proto
cargo test  # 运行所有测试
```

## 测试原则

1. **不需要真实硬件**：所有测试应在 CI 环境中运行，不依赖蓝牙适配器
2. **Mock 外部依赖**：BlueZ、网络接口、系统命令
3. **临时文件隔离**：每个测试使用独立临时目录
4. **确定性**：测试不依赖时序、网络状态

## 测试覆盖计划

| 模块 | 测试类型 | 优先级 |
|------|----------|--------|
| config | 单元+集成 | 高 |
| proto | 单元+属性测试 | 高 |
| wifi | 集成（Mock 命令） | 高 |
| custom_command | 单元+集成 | 中 |
| custom_info | 单元+集成 | 中 |
| ble/gatt | 集成（Mock D-Bus） | 中 |
| ble/bluez | 集成（Mock D-Bus） | 低 |

## CI 集成

```yaml
# .github/workflows/test.yml
name: Tests
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: dtolnay/rust-action@stable
      - run: cargo test --all-features
      - run: cargo test --doc
```
