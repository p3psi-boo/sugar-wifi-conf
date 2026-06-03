//! 自定义命令模块集成测试

use sugar_wifi_conf_rs::custom_command::{
    parse_custom_command_request, command_for_suffix, index0_from_suffix,
    CustomCommandRequest, CustomCommandParseError, CustomCommandItem
};
use sugar_wifi_conf_rs::proto::CONCAT_TAG;

/// 测试基本请求解析
#[test]
fn test_parse_basic_request() {
    let payload = b"pisugar%&%0001";
    let req = parse_custom_command_request(payload).unwrap();
    
    assert_eq!(req.key, "pisugar");
    assert_eq!(req.suffix, "0001");
}

/// 测试大写十六进制后缀
#[test]
fn test_parse_uppercase_suffix() {
    let payload = b"key%&%000A";
    let req = parse_custom_command_request(payload).unwrap();
    
    assert_eq!(req.suffix, "000A");
}

/// 测试小写十六进制后缀（应该转换为大写）
#[test]
fn test_parse_lowercase_suffix() {
    let payload = b"key%&%000a";
    let req = parse_custom_command_request(payload).unwrap();
    
    assert_eq!(req.suffix, "000A");
}

/// 测试完整 UUID 格式
#[test]
fn test_parse_full_uuid() {
    let payload = b"key%&%FD2BCCCC0001";
    let req = parse_custom_command_request(payload).unwrap();
    
    assert_eq!(req.suffix, "0001");
}

/// 测试带横线的 UUID 格式
#[test]
fn test_parse_dashed_uuid() {
    let payload = b"key%&%fd2bcccc-0001";
    let req = parse_custom_command_request(payload).unwrap();
    
    assert_eq!(req.suffix, "0001");
}

/// 测试复杂 UUID 格式
#[test]
fn test_parse_complex_uuid() {
    let payload = b"key%&%550e8400-e29b-41d4-a716-446655440001";
    let req = parse_custom_command_request(payload).unwrap();
    
    assert_eq!(req.suffix, "0001");
}

/// 测试无效 UTF-8
#[test]
fn test_parse_invalid_utf8() {
    let payload = vec![0x80, 0x81, 0xfe];
    let result = parse_custom_command_request(&payload);
    
    assert!(matches!(result, Err(CustomCommandParseError::InvalidUtf8)));
}

/// 测试缺少分隔符
#[test]
fn test_parse_missing_delimiter() {
    let payload = b"onlykey";
    let result = parse_custom_command_request(payload);
    
    assert!(matches!(result, Err(CustomCommandParseError::InvalidSyntax)));
}

/// 测试空 key
#[test]
fn test_parse_empty_key() {
    let payload = b"%&%0001";
    let result = parse_custom_command_request(payload);
    
    assert!(matches!(result, Err(CustomCommandParseError::InvalidSyntax)));
}

/// 测试空 suffix
#[test]
fn test_parse_empty_suffix() {
    let payload = b"key%&%";
    let result = parse_custom_command_request(payload);
    
    assert!(matches!(result, Err(CustomCommandParseError::InvalidSyntax)));
}

/// 测试无效 suffix（非十六进制）
#[test]
fn test_parse_invalid_suffix() {
    let payload = b"key%&%GGGG";
    let result = parse_custom_command_request(payload);
    
    assert!(matches!(result, Err(CustomCommandParseError::InvalidSuffix)));
}

/// 测试 suffix 太短
#[test]
fn test_parse_short_suffix() {
    let payload = b"key%&%123";
    let result = parse_custom_command_request(payload);
    
    assert!(matches!(result, Err(CustomCommandParseError::InvalidSuffix)));
}

/// 测试 suffix 转换函数
#[test]
fn test_index0_from_suffix() {
    // 1-based indexing: 0001 -> 0, 0002 -> 1
    assert_eq!(index0_from_suffix("0001"), Some(0));
    assert_eq!(index0_from_suffix("0002"), Some(1));
    assert_eq!(index0_from_suffix("000A"), Some(9));
    assert_eq!(index0_from_suffix("000a"), Some(9)); // 大小写不敏感
    assert_eq!(index0_from_suffix("FFFF"), Some(65534));
    
    // 无效情况
    assert_eq!(index0_from_suffix("0000"), None); // 0 无效
    assert_eq!(index0_from_suffix("GGGG"), None); // 非十六进制
    assert_eq!(index0_from_suffix("123"), None);  // 太短
}

/// 测试命令查找（通过索引）
#[test]
fn test_command_lookup_by_index() {
    let commands = vec![
        CustomCommandItem {
            label: "First".to_string(),
            command: "echo first".to_string(),
            uuid: None,
        },
        CustomCommandItem {
            label: "Second".to_string(),
            command: "echo second".to_string(),
            uuid: None,
        },
    ];
    
    assert_eq!(command_for_suffix(&commands, "0001"), Some("echo first"));
    assert_eq!(command_for_suffix(&commands, "0002"), Some("echo second"));
    assert_eq!(command_for_suffix(&commands, "0003"), None);
}

/// 测试命令查找（通过 UUID）
#[test]
fn test_command_lookup_by_uuid() {
    let commands = vec![
        CustomCommandItem {
            label: "First".to_string(),
            command: "echo first".to_string(),
            uuid: Some("ABCD".to_string()),
        },
        CustomCommandItem {
            label: "Second".to_string(),
            command: "echo second".to_string(),
            uuid: None,
        },
    ];
    
    assert_eq!(command_for_suffix(&commands, "ABCD"), Some("echo first"));
    assert_eq!(command_for_suffix(&commands, "0002"), Some("echo second"));
}

/// 测试命令查找（UUID 优先于索引）
#[test]
fn test_uuid_takes_precedence_over_index() {
    let commands = vec![
        CustomCommandItem {
            label: "With UUID".to_string(),
            command: "echo uuid".to_string(),
            uuid: Some("0002".to_string()), // 这个 UUID 看起来像第二个索引
        },
        CustomCommandItem {
            label: "Second".to_string(),
            command: "echo second".to_string(),
            uuid: None,
        },
    ];
    
    // 应该找到第一个命令（通过 UUID 匹配）
    assert_eq!(command_for_suffix(&commands, "0002"), Some("echo uuid"));
}

/// 测试请求相等性
#[test]
fn test_request_equality() {
    let req1 = CustomCommandRequest {
        key: "key".to_string(),
        suffix: "0001".to_string(),
    };
    let req2 = CustomCommandRequest {
        key: "key".to_string(),
        suffix: "0001".to_string(),
    };
    let req3 = CustomCommandRequest {
        key: "key".to_string(),
        suffix: "0002".to_string(),
    };
    
    assert_eq!(req1, req2);
    assert_ne!(req1, req3);
}

/// 测试大量命令的查找性能
#[test]
fn test_large_command_list() {
    let mut commands = Vec::new();
    for i in 0..100 {
        commands.push(CustomCommandItem {
            label: format!("Command {}", i),
            command: format!("echo {}", i),
            uuid: None,
        });
    }
    
    // 测试最后一个命令的查找
    assert_eq!(command_for_suffix(&commands, "0064"), Some("echo 99"));
}

/// 测试边界值后缀
#[test]
fn test_boundary_suffix_values() {
    let commands = vec![
        CustomCommandItem {
            label: "Only".to_string(),
            command: "echo only".to_string(),
            uuid: None,
        },
    ];
    
    // 边界值测试
    assert_eq!(command_for_suffix(&commands, "0001"), Some("echo only"));
    assert_eq!(command_for_suffix(&commands, "0000"), None); // 0 无效
    assert_eq!(command_for_suffix(&commands, "FFFF"), None); // 超出范围
}
