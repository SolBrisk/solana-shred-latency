# Solana Shred 测速对比工具

[English](README.md) · [SolBrisk 官网](https://solbrisk.com/) · [接入文档](https://solbrisk.com/docs)

同机同时接收 2～8 路原始 UDP Shred，比较同一条数据谁先到。
Linux 默认使用 **SO_TIMESTAMPNS 内核软件接收时间戳**，减少接收线程调度造成的偏差。
输出先到比例、时间差分位数、匹配量、重复、接收缓冲和队列丢包。
由 [SolBrisk](https://solbrisk.com/) 提供，也可对比其他兼容原始 Shred 的服务商。

## 安装与配置

需要 Rust 1.89+ 和 C 工具链，推荐 Linux；其他平台使用应用接收时间，包含额外调度
和 socket 排队影响。

```bash
cargo build --release --locked
cp examples/sources.json sources.local.json
```

示例配置：

```json
{
  "sources": [
    {"name":"solbrisk-fra","bind":"0.0.0.0:21001"},
    {"name":"other-provider","bind":"0.0.0.0:21002"}
  ]
}
```

将各家订阅分别推送到**测速服务器公网 IP:21001 / 21002**，不是 `0.0.0.0`。
防火墙放行对应 UDP 端口。可选填写 `source_ip` 限制发送方，地址需由服务商确认。
程序一次性独占绑定所有端口，不复用抢占已有服务，不修改防火墙、转发、系统服务
或 sysctl；实盘接收端已经占用的端口应换成新的测速端口。

需要法兰克福/阿姆斯特丹服务，可到 [SolBrisk 官网](https://solbrisk.com/)创建订阅，
按[接入文档](https://solbrisk.com/docs)配置目的端。程序能打印端点验证码并排除验证包；
建议先完成验证、确认各路已推送，再开始一轮新的测速。

## 开始测速

```bash
./target/release/solana-shred-latency \
  --sources sources.local.json --warmup 5 --duration 60 --output report.json
```

首个来源为基准，预热 5 秒，统计 60 秒，再等待默认 5 秒匹配窗口。
统计结束后只继续配对已经观察到的数据，不把全新的数据纳入测试。
最终 JSON 输出到 stdout，日志到 stderr，报告文件不会覆盖。

默认只测数据分片，`--include-coding` 加入编码分片。可用 `--userspace-timestamps`
明确切换到应用时间，但不能把它和内核时间戳的结果当作同一口径混用。

## 怎么看结果

**差值 = 对比来源到达时间 − 基准来源到达时间。**

- 正数基准先到，负数对比来源先到；先到比例只统计配对成功的数据。
- p50/p95/p99 是有正负号的时间差，不是从 Leader 发出开始的绝对延迟。
- `*_without_match` 为未匹配量，**不是准确网络丢包率**。
- `kernel_drop_count` 是 Linux 最近一次收到的 socket 溢出计数；最后一个收到的包
  之后若继续丢包，可能没有新的计数回传。它只反映本机 socket，不能等同公网丢包。
- `clean_run=false` 表示缺少匹配、中断、应用队列丢包、容量淘汰、接收错误、内核
  时间戳缺失或时钟偏移超阈值；退出非零但保留报告，先排除诊断问题再比较。
- 即使 `clean_run=true`，也不证明完整收到全部链上数据，不验证 Leader 签名。

完整方法见 [METHODOLOGY.md](METHODOLOGY.md)。建议交换基准，多轮测试并保留接收
地区、配置、时长、样本数及诊断字段，避免把单次样本变成服务商性能保证。

## 资源与验证

`--receive-buffer-mib` 请求 socket 缓冲，实际大小受操作系统限制，会在报告显示。
`--queue-capacity`、`--max-keys`、`--match-window`、`--quantile-samples` 可调。
无主动限速，也不调用 RPC；队列等待高或溢出说明可能在测本机瓶颈。

```bash
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

MIT 许可证，第三方说明见 [THIRD-PARTY.md](THIRD-PARTY.md)。
客服：[SolBrisk 接入指南](https://solbrisk.com/docs) / support@solbrisk.com。
