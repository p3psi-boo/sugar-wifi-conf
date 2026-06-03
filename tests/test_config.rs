//! 配置模块集成测试
//!
//! 测试配置文件解析和验证逻辑

use std::path::Path;

use sugar_wifi_conf_rs::config::{load_custom_config, CustomConfigFile};

mod helpers;
use helpers::TestContext;

/// 测试最小有效配置
#[test]
fn test_minimal_valid_config() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{
        "info": [],
        "commands": []
    }"#;
    
    let path = ctx.create_file("minimal.json", config_content).unwrap();
    let config = load_custom_config(&path).unwrap();
    
    assert!(config.info.is_empty());
    assert!(config.commands.is_empty());
}

/// 测试完整配置解析
#[test]
fn test_full_config_parsing() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{
        "info": [
            {
                "label": "CPU Temperature",
                "command": "cat /sys/class/thermal/thermal_zone0/temp",
                "interval": 5
            },
            {
                "label": "Uptime",
                "command": "uptime -p",
                "interval": 60
            }
        ],
        "commands": [
            {
                "label": "Restart Service",
                "command": "systemctl restart myservice",
                "uuid": "FD2B0001"
            },
            {
                "label": "Update System",
                "command": "apt update && apt upgrade -y"
            }
        ]
    }"#;
    
    let path = ctx.create_file("full.json", config_content).unwrap();
    let config = load_custom_config(&path).unwrap();
    
    // 验证 info 项
    assert_eq!(config.info.len(), 2);
    assert_eq!(config.info[0].label, "CPU Temperature");
    assert_eq!(config.info[0].command, "cat /sys/class/thermal/thermal_zone0/temp");
    assert_eq!(config.info[0].interval, Some(5));
    
    assert_eq!(config.info[1].label, "Uptime");
    assert_eq!(config.info[1].interval, Some(60));
    
    // 验证 commands
    assert_eq!(config.commands.len(), 2);
    assert_eq!(config.commands[0].label, "Restart Service");
    assert_eq!(config.commands[0].uuid, Some("FD2B0001".to_string()));
    
    assert_eq!(config.commands[1].label, "Update System");
    assert_eq!(config.commands[1].uuid, None);
}

/// 测试向后兼容：items 别名
#[test]
fn test_backward_compat_items_alias() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{
        "items": [
            {"label": "Test", "command": "echo test"}
        ],
        "commands": []
    }"#;
    
    let path = ctx.create_file("compat.json", config_content).unwrap();
    let config = load_custom_config(&path).unwrap();
    
    assert_eq!(config.info.len(), 1);
    assert_eq!(config.info[0].label, "Test");
}

/// 测试默认 interval 值
#[test]
fn test_default_interval() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{
        "info": [{"label": "Test", "command": "echo test"}],
        "commands": []
    }"#;
    
    let path = ctx.create_file("no_interval.json", config_content).unwrap();
    let config = load_custom_config(&path).unwrap();
    
    assert_eq!(config.info[0].interval, None);
}

/// 测试无效 JSON 错误处理
#[test]
fn test_invalid_json_error() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{ invalid json }"#;
    
    let path = ctx.create_file("invalid.json", config_content).unwrap();
    let result = load_custom_config(&path);
    
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("parse") || err_msg.contains("JSON"));
}

/// 测试缺失必需字段
#[test]
fn test_missing_required_field() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{
        "info": [{"label": "Missing Command"}],
        "commands": []
    }"#;
    
    let path = ctx.create_file("missing_field.json", config_content).unwrap();
    let result = load_custom_config(&path);
    
    assert!(result.is_err());
}

/// 测试空文件
#[test]
fn test_empty_file() {
    let ctx = TestContext::new().unwrap();
    let path = ctx.create_file("empty.json", "").unwrap();
    let result = load_custom_config(&path);
    
    assert!(result.is_err());
}

/// 测试不存在的文件
#[test]
fn test_nonexistent_file() {
    let path = Path::new("/nonexistent/path/config.json");
    let result = load_custom_config(path);
    
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("read"));
}

/// 测试特殊字符处理
#[test]
fn test_special_characters_in_fields() {
    let ctx = TestContext::new().unwrap();
    let config_content = r#"{
        "info": [
            {
                "label": "Test: Special \"Chars\"",
                "command": "echo 'Hello\\nWorld'"
            }
        ],
        "commands": []
    }"#;
    
    let path = ctx.create_file("special.json", config_content).unwrap();
    let config = load_custom_config(&path).unwrap();
    
    assert_eq!(config.info[0].label, "Test: Special \"Chars\"");
    assert_eq!(config.info[0].command, "echo 'Hello\\nWorld'");
}

/// 测试大型配置性能
#[test]
fn test_large_config_performance() {
    let ctx = TestContext::new().unwrap();
    
    // 生成大量 info 项
    let mut info_items = Vec::new();
    for i in 0..100 {
        info_items.push(format!(
            r#"{{"label": "Item{}", "command": "echo {}", "interval": {}}}"#,
            i, i, i % 60 + 1
        ));
    }
    
    let config_content = format!(
        r#"{{"info": [{}], "commands": []}}"#,
        info_items.join(",")
    );
    
    let path = ctx.create_file("large.json", &config_content).unwrap();
    
    let start = std::time::Instant::now();
    let config = load_custom_config(&path).unwrap();
    let elapsed = start.elapsed();
    
    assert_eq!(config.info.len(), 100);
    assert!(elapsed < std::time::Duration::from_secs(1), "Config parsing too slow");
}

/// 测试配置验证：危险命令检测（建议添加的功能）
#[test]
#[ignore = "待实现配置验证功能"]
fn test_dangerous_command_detection() {
    // 此测试待实现，用于验证配置验证功能
    // 应该检测如 "rm -rf /"、"> /dev/sda" 等危险命令
}
