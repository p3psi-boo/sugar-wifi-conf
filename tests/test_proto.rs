//! 协议层集成测试
//!
//! 测试 BLE 数据包重组、分帧逻辑

use sugar_wifi_conf_rs::proto::{
    frame_notify_message, ReassemblyBuffer, CONCAT_TAG, END_TAG, 
    DEFAULT_MAX_REASSEMBLY_BYTES, NOTIFY_CHUNK_SIZE, NOTIFY_CHUNK_DELAY_MS
};

/// 测试基本重组功能
#[test]
fn test_basic_reassembly() {
    let mut buf = ReassemblyBuffer::new(1024);
    
    let result = buf.push(b"hello world&#&").unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], b"hello world");
    assert!(buf.is_empty());
}

/// 测试跨边界重组
#[test]
fn test_cross_boundary_reassembly() {
    let mut buf = ReassemblyBuffer::new(1024);
    
    // 分割 END_TAG 跨两次 push
    let out1 = buf.push(b"hello&").unwrap();
    assert!(out1.is_empty());
    assert_eq!(buf.len(), 6);
    
    let out2 = buf.push(b"#&").unwrap();
    assert_eq!(out2.len(), 1);
    assert_eq!(out2[0], b"hello");
    assert!(buf.is_empty());
}

/// 测试多消息一次性重组
#[test]
fn test_multiple_messages_at_once() {
    let mut buf = ReassemblyBuffer::new(1024);
    
    let result = buf.push(b"msg1&#&msg2&#&msg3&#&").unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], b"msg1");
    assert_eq!(result[1], b"msg2");
    assert_eq!(result[2], b"msg3");
    assert!(buf.is_empty());
}

/// 测试部分消息保留
#[test]
fn test_partial_message_retention() {
    let mut buf = ReassemblyBuffer::new(1024);
    
    let out1 = buf.push(b"complete&#&incomplete").unwrap();
    assert_eq!(out1.len(), 1);
    assert_eq!(out1[0], b"complete");
    assert_eq!(buf.len(), 11); // "incomplete"
    
    let out2 = buf.push(b" now&#&").unwrap();
    assert_eq!(out2.len(), 1);
    assert_eq!(out2[0], b"incomplete now");
    assert!(buf.is_empty());
}

/// 测试空 push
#[test]
fn test_empty_push() {
    let mut buf = ReassemblyBuffer::new(1024);
    
    let result = buf.push(b"").unwrap();
    assert!(result.is_empty());
    
    // 空 push 不应该影响后续重组
    buf.push(b"test&#&").unwrap();
    let result = buf.push(b"").unwrap();
    assert!(result.is_empty());
}

/// 测试溢出处理
#[test]
fn test_overflow_handling() {
    let mut buf = ReassemblyBuffer::new(10);
    
    // 超过限制应该报错并清空
    let result = buf.push(b"01234567890");
    assert!(result.is_err());
    assert!(buf.is_empty());
    
    // 清空后应该可以正常使用
    let result = buf.push(b"ok&#&").unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], b"ok");
}

/// 测试默认最大大小
#[test]
fn test_default_max_size() {
    let buf = ReassemblyBuffer::with_default_max();
    assert_eq!(buf.len(), 0);
    // 默认大小是 16KB
    assert_eq!(DEFAULT_MAX_REASSEMBLY_BYTES, 16 * 1024);
}

/// 测试超大消息
#[test]
fn test_large_message() {
    let mut buf = ReassemblyBuffer::new(64 * 1024);
    
    let large_payload = vec![b'x'; 10000];
    let mut message = large_payload.clone();
    message.extend_from_slice(END_TAG.as_bytes());
    
    let result = buf.push(&message).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].len(), 10000);
}

/// 测试通知消息分帧
#[test]
fn test_frame_notify_message() {
    let message = b"hello";
    let chunks = frame_notify_message(message);
    
    // 小消息应该只有一块
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], b"hello&#&");
}

/// 测试通知消息分块
#[test]
fn test_notify_message_chunking() {
    // 创建超过 20 字节的消息
    let message = vec![b'a'; 25];
    let chunks = frame_notify_message(&message);
    
    // 应该分成多块
    assert!(chunks.len() > 1);
    
    // 第一块应该是 20 字节
    assert_eq!(chunks[0].len(), NOTIFY_CHUNK_SIZE);
    
    // 所有块连接起来应该包含原始消息和结束标记
    let combined: Vec<u8> = chunks.concat();
    assert!(combined.starts_with(&message));
    assert!(combined.ends_with(END_TAG.as_bytes()));
}

/// 测试精确 20 字节边界
#[test]
fn test_exact_chunk_boundary() {
    let message = vec![b'b'; NOTIFY_CHUNK_SIZE];
    let chunks = frame_notify_message(&message);
    
    // 20 字节 payload + 3 字节 END_TAG = 23 字节
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].len(), NOTIFY_CHUNK_SIZE);
    assert_eq!(chunks[1].len(), END_TAG.len());
}

/// 测试延迟常量
#[test]
fn test_chunk_delay_constant() {
    assert_eq!(NOTIFY_CHUNK_DELAY_MS, 200);
}

/// 测试 CONCAT_TAG 格式（用于 WiFi 请求）
#[test]
fn test_concat_tag_format() {
    assert_eq!(CONCAT_TAG, "%&%");
    
    // 模拟 WiFi 请求格式：key%&%ssid%&%password
    let request = format!("pisugar{}MyWiFi{}password123", CONCAT_TAG, CONCAT_TAG);
    let parts: Vec<&str> = request.split(CONCAT_TAG).collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], "pisugar");
    assert_eq!(parts[1], "MyWiFi");
    assert_eq!(parts[2], "password123");
}

/// 测试连续流式数据
#[test]
fn test_streaming_data() {
    let mut buf = ReassemblyBuffer::new(1024);
    let mut received = Vec::new();
    
    // 模拟网络数据包流
    let packets = vec![
        b"msg1&#&msg".as_slice(),
        b"2&#&".as_slice(),
        b"msg3&#&msg4&#&".as_slice(),
    ];
    
    for packet in packets {
        let msgs = buf.push(packet).unwrap();
        for msg in msgs {
            received.push(String::from_utf8(msg).unwrap());
        }
    }
    
    assert_eq!(received, vec!["msg1", "msg2", "msg3", "msg4"]);
}

/// 测试缓冲区状态检查
#[test]
fn test_buffer_state() {
    let mut buf = ReassemblyBuffer::new(100);
    
    assert!(buf.is_empty());
    assert_eq!(buf.len(), 0);
    
    buf.push(b"partial").unwrap();
    assert!(!buf.is_empty());
    assert_eq!(buf.len(), 7);
    
    buf.push(b" data&#&").unwrap();
    assert!(buf.is_empty());
}

/// 测试手动清空
#[test]
fn test_manual_clear() {
    let mut buf = ReassemblyBuffer::new(100);
    buf.push(b"partial data").unwrap();
    
    assert!(!buf.is_empty());
    buf.clear();
    assert!(buf.is_empty());
    assert_eq!(buf.len(), 0);
}

/// 压力测试：大量小消息
#[test]
fn test_stress_many_small_messages() {
    let mut buf = ReassemblyBuffer::new(1024 * 1024);
    let message_count = 1000;
    
    // 构造消息流
    let mut stream = Vec::new();
    for i in 0..message_count {
        stream.extend_from_slice(format!("msg{}&#&", i).as_bytes());
    }
    
    let result = buf.push(&stream).unwrap();
    assert_eq!(result.len(), message_count);
}

/// 测试无效 UTF-8 数据（应该正常处理，因为 proto 层处理原始字节）
#[test]
fn test_invalid_utf8_data() {
    let mut buf = ReassemblyBuffer::new(1024);
    
    // 包含无效 UTF-8 序列
    let data = vec![0x80, 0x81, 0x82, 0x83];
    let mut message = data.clone();
    message.extend_from_slice(END_TAG.as_bytes());
    
    let result = buf.push(&message).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], data);
}
