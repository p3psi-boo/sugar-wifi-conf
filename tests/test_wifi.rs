//! WiFi 模块集成测试
//!
//! 测试 WiFi 请求解析和配置逻辑

use sugar_wifi_conf_rs::wifi::{
    parse_wifi_request, WifiRequest, ParseWifiRequestError, 
    detect_wifi_interface_inner, DEFAULT_WIFI_INTERFACE
};
use sugar_wifi_conf_rs::proto::CONCAT_TAG;

/// 测试有效 WiFi 请求解析
#[test]
fn test_parse_valid_wifi_request() {
    let payload = b"pisugar%&%MyWiFi%&%password123";
    let req = parse_wifi_request(payload).unwrap();
    
    assert_eq!(req.key, "pisugar");
    assert_eq!(req.ssid, "MyWiFi");
    assert_eq!(req.password, "password123");
}

/// 测试包含空格和特殊字符的 SSID
#[test]
fn test_parse_wifi_request_with_special_chars() {
    let payload = b"key%&%My WiFi 2.4G%&%pass with spaces!@#";
    let req = parse_wifi_request(payload).unwrap();
    
    assert_eq!(req.key, "key");
    assert_eq!(req.ssid, "My WiFi 2.4G");
    assert_eq!(req.password, "pass with spaces!@#");
}

/// 测试空密码
#[test]
fn test_parse_wifi_request_empty_password() {
    let payload = b"key%&%OpenNetwork%&%";
    let req = parse_wifi_request(payload).unwrap();
    
    assert_eq!(req.ssid, "OpenNetwork");
    assert_eq!(req.password, "");
}

/// 测试无效 UTF-8
#[test]
fn test_parse_invalid_utf8() {
    let payload = vec![0x80, 0x81, 0x82];
    let result = parse_wifi_request(&payload);
    
    assert!(matches!(result, Err(ParseWifiRequestError::InvalidUtf8)));
}

/// 测试缺少分隔符
#[test]
fn test_parse_missing_delimiter() {
    let payload = b"onlykeyandssid";
    let result = parse_wifi_request(payload);
    
    assert!(matches!(result, Err(ParseWifiRequestError::InvalidSyntax)));
}

/// 测试只有一个分隔符
#[test]
fn test_parse_single_delimiter() {
    let payload = b"key%&%ssid";
    let result = parse_wifi_request(payload);
    
    assert!(matches!(result, Err(ParseWifiRequestError::InvalidSyntax)));
}

/// 测试多余字段（应该忽略）
#[test]
fn test_parse_extra_fields_ignored() {
    let payload = b"key%&%ssid%&%pass%&%extra%&%more";
    let req = parse_wifi_request(payload).unwrap();
    
    assert_eq!(req.key, "key");
    assert_eq!(req.ssid, "ssid");
    assert_eq!(req.password, "pass");
}

/// 测试请求相等性
#[test]
fn test_wifi_request_equality() {
    let req1 = WifiRequest {
        key: "key".to_string(),
        ssid: "ssid".to_string(),
        password: "pass".to_string(),
    };
    let req2 = WifiRequest {
        key: "key".to_string(),
        ssid: "ssid".to_string(),
        password: "pass".to_string(),
    };
    let req3 = WifiRequest {
        key: "other".to_string(),
        ssid: "ssid".to_string(),
        password: "pass".to_string(),
    };
    
    assert_eq!(req1, req2);
    assert_ne!(req1, req3);
}

/// 测试 CONCAT_TAG 常量
#[test]
fn test_concat_tag() {
    assert_eq!(CONCAT_TAG, "%&%");
}

/// 测试默认 WiFi 接口名
#[test]
fn test_default_wifi_interface() {
    assert_eq!(DEFAULT_WIFI_INTERFACE, "wlan0");
}

/// 测试长 SSID 和密码
#[test]
fn test_long_ssid_and_password() {
    let long_ssid = "a".repeat(32);  // 最大 SSID 长度
    let long_password = "b".repeat(63);  // 常见最大密码长度
    
    let payload = format!("key%&%{}%&%{}" , long_ssid, long_password);
    let req = parse_wifi_request(payload.as_bytes()).unwrap();
    
    assert_eq!(req.ssid, long_ssid);
    assert_eq!(req.password, long_password);
}

/// 测试 Unicode SSID
#[test]
fn test_unicode_ssid() {
    let payload = "key%&%我的WiFi%&%密码123";
    let req = parse_wifi_request(payload.as_bytes()).unwrap();
    
    assert_eq!(req.ssid, "我的WiFi");
    assert_eq!(req.password, "密码123");
}

// ========== 以下测试需要模拟外部命令，暂时标记为忽略 ==========

/// 测试 WiFi 接口检测（需要模拟 /sys/class/net）
#[test]
#[ignore = "需要模拟文件系统"]
fn test_detect_wifi_interface() {
    // 待实现：使用 tempfs 模拟 /sys/class/net
}

/// 测试 WPA 配置文件更新（需要模拟文件系统）
#[test]
#[ignore = "需要模拟文件系统"]
fn test_update_wpa_supplicant_conf() {
    // 待实现：测试配置文件的更新逻辑
}

/// 测试网络接口检查（需要模拟 /etc/network/interfaces）
#[test]
#[ignore = "需要模拟文件系统"]
fn test_is_interface_ok() {
    // 待实现：测试网络接口配置检查
}

/// 测试配置WiFi完整流程（集成测试）
#[tokio::test]
#[ignore = "需要重构模块以支持依赖注入"]
async fn test_configure_wifi_full_flow() {
    // 待实现：模拟完整的WiFi配置流程
    // 需要重构 wifi 模块以支持命令模拟
}
