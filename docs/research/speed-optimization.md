# nim-proxy 速度优化调研

> 历史设计草稿；示例耗时与收益百分比是推演，不是实测证据。
> 当前实现范围和已知限制见 [模型竞速说明](../features/model-racing.md)。

## 需求澄清

**目标**: 降低 nim-proxy 的模型响应延迟，提升首字时间 (TTFT)

**方向**:
1. 模型竞速 (Model Racing) - 多模型并发，首个响应获胜
2. 自动权重分配 - 根据任务复杂度选择最优模型
3. 其他速度优化技术

---

## 当前 nim-proxy 架构分析

### 现有性能特性

从 README 可知，nim-proxy 当前已实现：
- ✅ 速率限制管理 (40 RPM/key)
- ✅ 多 key 负载均衡
- ✅ 会话亲和性 (prefix-cache 优化)
- ✅ 重试和 Retry-After 处理
- ✅ 模型worker并发上限管理
- ✅ SSE 心跳保活

### 性能瓶颈分析

**当前可能的延迟来源**:
1. **上游 NIM API 响应时间** - 主要瓶颈
2. **等待队列** - RPM 限制导致的排队
3. **单模型依赖** - 只等待一个模型响应
4. **重试延迟** - 429/5xx 错误后的重试

---

## 方案 1: 模型竞速 (Model Racing)

### 核心概念

并发请求多个模型，返回首个成功响应，取消其他请求。

### 实现方案

#### A. 完全并行竞速
```rust
// 伪代码
async fn race_models(
    request: ChatRequest,
    models: Vec<String>,
) -> Result<ChatResponse> {
    // 并发请求所有模型
    let futures: Vec<_> = models.iter()
        .map(|model| {
            let mut req = request.clone();
            req.model = model.clone();
            send_to_nim(req)
        })
        .collect();
    
    // 返回首个成功响应
    let (result, _index, remaining) = select_ok(futures).await?;
    
    // 取消其他请求（释放资源）
    for fut in remaining {
        fut.cancel();
    }
    
    Ok(result)
}
```

**优点**:
- ✅ 最小化 TTFT (首字时间)
- ✅ 自动容错 (某个模型慢/失败不影响)
- ✅ 隐式负载均衡

**缺点**:
- ❌ **Key 消耗放大** (N个模型 = N倍消耗)
- ❌ **RPM 压力** (更快耗尽速率限制)
- ❌ **浪费计算** (取消的请求已消耗资源)
- ❌ **成本增加** (如果按使用量计费)

#### B. 智能竞速策略

```rust
struct RaceStrategy {
    primary_model: String,
    fallback_models: Vec<String>,
    primary_timeout_ms: u64,
}

async fn smart_race(
    request: ChatRequest,
    strategy: RaceStrategy,
) -> Result<ChatResponse> {
    // 先尝试主模型
    let primary = send_to_nim(&strategy.primary_model, &request);
    
    // 设置超时
    match timeout(
        Duration::from_millis(strategy.primary_timeout_ms),
        primary
    ).await {
        Ok(Ok(response)) => Ok(response),
        _ => {
            // 超时或失败，竞速备选模型
            race_models(request, strategy.fallback_models).await
        }
    }
}
```

**优点**:
- ✅ 正常情况下不增加消耗
- ✅ 仅在需要时启动竞速
- ✅ 可配置的超时阈值

**缺点**:
- ⚠️ 配置复杂度
- ⚠️ 需要合理的超时设置

#### C. 首字竞速 (TTFT Racing)

**最佳方案**：只竞速到首字，然后取消其他

```rust
async fn ttft_race(
    request: ChatRequest,
    models: Vec<String>,
) -> Result<impl Stream<Item = SseEvent>> {
    let streams: Vec<_> = models.iter()
        .map(|model| {
            let mut req = request.clone();
            req.model = model.clone();
            req.stream = true;
            stream_from_nim(req)
        })
        .collect();
    
    // 竞速到首个 token
    let (first_token, winner_stream, _) = race_to_first_token(streams).await?;
    
    // 返回获胜者的流，取消其他
    Ok(winner_stream)
}
```

**优点**:
- ✅ **最优延迟** - 只竞速到首字
- ✅ **资源节省** - 首字后立即取消其他
- ✅ **用户体验最佳** - TTFT 是感知延迟的关键

**缺点**:
- ⚠️ 实现复杂（SSE 流处理）
- ⚠️ 仍有首字前的消耗

### 性能提升预估

假设 3 个模型并发：
- 模型 A: TTFT 2000ms
- 模型 B: TTFT 1500ms
- 模型 C: TTFT 1800ms

**无竞速**: 平均 TTFT = 1766ms
**竞速**: TTFT = min(2000, 1500, 1800) = **1500ms**

**理论加速**: 15-30% (取决于模型速度差异)

### 成本分析

**Key 消耗**:
- 完全并行: N倍
- 智能竞速: 1.2-1.5倍 (大部分请求命中主模型)
- 首字竞速: 1.5-2倍 (首字前的消耗)

**ROI 评估**:
- 如果速度是关键需求，15-30% 的延迟降低值得 1.5-2x 成本
- 对于批量任务，不推荐
- 对于交互式使用，强烈推荐

---

## 方案 2: 自动模型选择 (Smart Model Selection)

### 核心概念

根据请求特征自动选择最快/最合适的模型。

### 实现方案

#### A. 基于规则的选择

```rust
struct ModelSelector {
    rules: Vec<SelectionRule>,
}

struct SelectionRule {
    condition: RequestCondition,
    preferred_model: String,
}

enum RequestCondition {
    MessageCountLessThan(usize),
    TotalTokensLessThan(usize),
    HasTools(bool),
    HasImages(bool),
    // ...
}

impl ModelSelector {
    fn select_model(&self, request: &ChatRequest) -> String {
        // 简单任务 → 快速小模型
        if request.messages.len() <= 3 && !request.has_tools() {
            return "mistral-7b-instruct".to_string();
        }
        
        // 复杂任务 → 强力大模型
        if request.has_tools() || request.messages.len() > 10 {
            return "llama-3.1-405b-instruct".to_string();
        }
        
        // 默认中等模型
        "llama-3.1-70b-instruct".to_string()
    }
}
```

**优点**:
- ✅ 简单直接
- ✅ 可预测
- ✅ 无额外成本

**缺点**:
- ❌ 规则难以精确
- ❌ 需要人工调优
- ❌ 无法适应模型性能变化

#### B. 基于历史数据的选择

```rust
struct PerformanceTracker {
    model_stats: HashMap<String, ModelStats>,
}

struct ModelStats {
    avg_ttft: Duration,
    avg_tokens_per_sec: f64,
    success_rate: f64,
    avg_quality_score: f64, // 用户反馈
}

impl PerformanceTracker {
    fn recommend_model(&self, request: &ChatRequest) -> String {
        // 根据历史性能选择
        let candidates = self.get_suitable_models(request);
        
        candidates.iter()
            .min_by_key(|model| {
                let stats = &self.model_stats[*model];
                // 综合评分：速度 + 质量 + 成功率
                (stats.avg_ttft.as_millis() as f64 * 0.5)
                    + (1.0 / stats.avg_tokens_per_sec * 0.3)
                    + ((1.0 - stats.success_rate) * 1000.0 * 0.2)
            })
            .cloned()
            .unwrap()
    }
}
```

**优点**:
- ✅ 自适应
- ✅ 基于实际性能
- ✅ 可持续优化

**缺点**:
- ⚠️ 需要收集足够数据
- ⚠️ 冷启动问题
- ⚠️ 实现复杂度较高

#### C. 机器学习模型选择

使用轻量级 ML 模型预测最优选择。

**特征**:
- 消息数量
- 总 token 数
- 是否有 tools
- 是否有图片
- 历史模型性能
- 当前负载情况

**训练**:
- 收集历史请求数据
- 标签：实际最快的模型
- 使用简单的决策树或 XGBoost

**优点**:
- ✅ 最优化的选择
- ✅ 考虑多维因素

**缺点**:
- ❌ 实现复杂
- ❌ 需要训练数据
- ❌ 推理开销（虽然很小）

### 性能提升预估

- 简单规则: 10-20% 平均延迟降低
- 历史数据: 15-25% 平均延迟降低
- ML 模型: 20-30% 平均延迟降低

---

## 方案 3: 其他速度优化

### A. 连接池和 Keep-Alive

```rust
// 复用 HTTP 连接
let client = reqwest::Client::builder()
    .pool_max_idle_per_host(10)
    .tcp_keepalive(Duration::from_secs(60))
    .build()?;
```

**预期提升**: 5-10% (减少 TCP 握手)

### B. 预测性预热

```rust
// 根据会话历史预测下一个请求，提前发送
async fn predictive_warmup(session: &Session) {
    if should_warmup(session) {
        let predicted_request = predict_next_request(session);
        // 提前建立连接，预加载模型
        warmup_connection(&predicted_request.model).await;
    }
}
```

**预期提升**: 10-20% (对于可预测的模式)

### C. 响应流优化

```rust
// 立即开始流式传输，不等待完整响应
async fn stream_optimized(request: ChatRequest) -> impl Stream<Item = SseEvent> {
    let mut stream = send_stream_request(request).await?;
    
    // 立即转发首个 token，不缓冲
    stream.map(|event| {
        // 零拷贝转发
        event
    })
}
```

**预期提升**: 减少缓冲延迟 50-100ms

### D. 并行请求处理

当前如果是串行处理请求：
```rust
// 改为并发处理独立请求
tokio::spawn(async move {
    handle_request(req).await
});
```

---

## 综合推荐方案

### 短期（立即实施）

1. **连接池优化** - 工作量小，立即见效
2. **响应流优化** - 减少不必要的缓冲
3. **简单规则的模型选择** - 容易实现

**预期总提升**: 20-30%
**实施时间**: 1-2 天

### 中期（1-2周）

4. **智能竞速 (首字竞速)** - 核心功能
5. **历史性能追踪** - 支持自适应选择

**预期总提升**: 35-45%
**实施时间**: 1-2 周

### 长期（1个月+）

6. **ML 模型选择** - 最优化
7. **预测性预热** - 高级优化

**预期总提升**: 50%+
**实施时间**: 1 个月

---

## 实施优先级

### P0 (必须有)
- ✅ 模型竞速基础框架
- ✅ 配置化的竞速策略

### P1 (强烈建议)
- ⭐ 首字竞速 (TTFT Racing)
- ⭐ 简单规则模型选择
- ⭐ 连接池优化

### P2 (长期改进)
- 📊 历史性能追踪
- 📊 自适应模型选择
- 📊 预测性预热

---

## 下一步行动

你希望我：

1. **开始实现模型竞速** - 从首字竞速开始
2. **实现自动模型选择** - 基于规则或历史数据
3. **都实现** - 完整的速度优化方案
4. **其他方向** - 你有其他想法？

请告诉我你的优先级，我会专注于实现你最关心的功能！
