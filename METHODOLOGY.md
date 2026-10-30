# Measurement methodology / 测量口径

## Match identical logical data

Data keys hash the leader signature, slot, shred index, type, version, FEC start,
parent offset, flags, declared size and actual entry-bearing bytes. Padding,
Merkle tails, retransmitter signatures and optional repair nonces are not used.
This tolerates differing relay tail representations while preserving application
payload and signed-batch identity. Coding keys use the header and erasure bytes.
All keys use SHA-256. Conflicting actual payloads or leader signatures do not
match. Grouping only by slot or comparing raw counts is insufficient.

The parser checks current chained Merkle header layout, geometry and lengths.
It does not authenticate leader signatures or establish the canonical fork.
Timestamps from traffic on a spoofed UDP source cannot be treated as trusted.

## Timing

Linux defaults to `recvmsg` with SO_TIMESTAMPNS and SO_RXQ_OVFL. Software-ingress
timestamps use receiver CLOCK_REALTIME. One fixed shared mapping translates them
to the process's Instant basis, allowing identical comparison/measurement-window
logic. A sampled difference over 5ms between realtime and monotonic elapsed time
invalidates the run; this can reflect a clock step or substantial accumulated
slew. Missing/truncated ancillary timestamp data also invalidates it. No hardware
NIC timestamping or sender timestamps are used. Both sources share the receiver
clock; source-server clock synchronization is unnecessary.

Userspace mode records Instant immediately after recv_from, before parsing and
hashing. Its numbers include socket buffering and thread scheduling, so do not
present them as pure network latency or compare them directly with kernel-mode
figures. Even kernel timestamps depend on receiver NIC/driver/network stack.

All sockets bind before worker startup. Warmup is excluded. Measurement admits
new keys during its duration and then drains counterparts of admitted keys for
one matching window. First arrivals per source within the retention window are
used; duplicates do not generate additional matched pairs. Identity retention
expires on local processing time. Large application queue lag can affect window
handling and must be disclosed. A queue overflow is counted, not silently ignored.

## Statistics

Each candidate compares independently against source 0. Signed differences are
candidate minus baseline in microseconds. Positive means baseline first.
Counts, mean and extrema cover all accepted matched pairs. Quantiles use every
pair until the configurable sample capacity (default 100,000), then fixed-size
reservoir sampling; the report explicitly identifies sampled quantiles.

Unmatched identities may reflect boundaries, different filtering/forks, provider
omissions, lost traffic, missing counterpart packets or local capacity. There is
no complete expected sequence from an authenticated sender, so **unmatched
percentage is not a measured network packet-loss percentage**. SO_RXQ_OVFL
reports local receive-queue drops only, and only as observed on subsequently
received datagrams. It cannot prove that a silent source has zero loss.

Use adequate receiver resources, equal source subscription settings and repeated
runs. Record location, UTC start, event selection, buffer sizes, sample count and
all diagnostics. `clean_run` is a local diagnostic gate, not proof of complete
coverage, statistically meaningful superiority or secure input authentication.

## 中文要点

同机对比实际相同的载荷，排除中继可变尾部，保留签名及业务字段区分分叉/冲突。
默认 Linux 内核软件接收时间戳，不使用发送端时钟。正数表示基准先到，负数表示对比
来源先到；分位数是相对差值。接收队列溢出和未匹配量各有含义，不能直接宣称公网丢包率。
对比前让各路订阅稳定，使用新端口，测试多轮并披露本机开销与统计采样方式。

Compatible native UDP setup: [SolBrisk documentation](https://solbrisk.com/docs).
