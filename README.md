# ts3web —— 让临时队友用浏览器连上你的 TS3 服务器

**当前状态：已部署并验证通过**（NAS `10.0.0.123`，端口 `18080`）

| 项目 | 值 |
|---|---|
| 网页（内网） | `https://home.inory.xin:18080/?n=昵称` |
| 网页（外网） | `https://inory.xin:18080/?n=昵称` ← **需要先在路由器加映射**（外部 18080/TCP → 10.0.0.123:18080） |
| 访问口令 | **当前未设置**（谁拿到链接都能进）。想限制：在 `/opt/ts3web/.env` 里填 `TS3WEB_TOKEN=xxx` 后 `docker compose up -d`，链接改成 `?t=xxx` |
| 默认频道 | `刑满释放`（改 `/opt/ts3web/.env` 的 `TS3WEB_CHANNEL` 后 `docker compose up -d`） |
| NAS 上的路径 | `/opt/ts3web`（容器名 `ts3web`，只监听 `127.0.0.1:18080`） |
| HTTPS 入口 | `/opt/1panel/www/conf.d/zz-https-ports.conf` 里的 18080 段（用 `/root/add-https-service.sh` 生成） |

## 怎么工作的

```
浏览器（getUserMedia 采集 + WebCodecs 里 AudioEncoder/Decoder 编解码 Opus，20ms 一帧）
    │  wss://home.inory.xin:18080/ws?t=口令&n=昵称
    ▼
OpenResty（10.0.0.123:18080 做 TLS 终结，复用 inory.xin 通配证书）
    │  http://127.0.0.1:18080
    ▼
ts3web 容器（Rust 单二进制：自己发网页 + WebSocket + tsclientlib 实现 TS3 客户端协议）
    │  UDP 9987（1panel-network 里直连 1Panel-teamspeak-nn9k）
    ▼
TS3 服务器（自己负责混音，本服务不做混音）
```

每个网页用户 = **一个独立的 TS3 身份**（未注册访客），所以 N 个网页用户就是 N 个轻量客户端。

## 关键设计选择

- **不用真 WebRTC**：浏览器只会 WebRTC，而 WebRTC 要路由器转发 UDP、必要时还要 TURN，CGNAT/公司网下经常连不上。
  改用 `wss://`（只走 443/18080 一个 TCP 口）+ WebCodecs 里直接编解码 Opus，**端到端零重编码**（TS3 的 OpusVoice 也是 48kHz/20ms）。
- **服务端只做搬运**：收到 TS3 的 Opus 包原样转发给浏览器；浏览器发来的 Opus 包原样塞回 TS3。
  唯一要做的是解析 Opus 的 TOC 字节算出帧时长（`src/opus.rs`），因为浏览器的 `AudioDecoder` 需要 `duration`。
- **每个说话人一个解码器**：服务端→浏览器的帧带 3 字节头 `[u16 speaker_id][u8 时长ms]`。
  Opus 解码器是有状态的，不同说话人共用解码器会有杂音，所以浏览器侧按 speaker_id 分别建 `AudioDecoder`。
- **编译不在 NAS 上做**：NAS 只有 1 个核。用本机 WSL（32 核）编译，NAS 只负责打包镜像。

## 目录

| 文件 | 作用 |
|---|---|
| `src/main.rs` | 配置（环境变量）、日志、接受连接 |
| `src/http.rs` | 内嵌网页 + 极简 HTTP；`peek` 判断是 WebSocket 还是普通 GET |
| `src/session.rs` | 核心：WebSocket ↔ TS3 双向音频、hello 握手、口令校验 |
| `src/opus.rs` | 解析 Opus TOC 算帧时长 |
| `src/selftest.rs` | 自检模式：不靠浏览器就能验证 TS3 连接/频道/发言 |
| `web/index.html` | 浏览器端（采集、编码、分说话人解码、抖动缓冲、界面） |
| `build-wsl.sh` | 在本机 WSL 里编译，产物落到 `bin/ts3web` |
| `deploy/deploy-from-pc.sh` | 把二进制推到 NAS、在 NAS 上重建镜像并重启 |
| `deploy/install.sh` | NAS 上首次安装：生成口令、起容器、调 `add-https-service.sh` |
| `deploy/ws-selftest.py` | Python 版客户端：不用浏览器验证整条 WebSocket 链路 |
| `deploy/run-ws-test.sh` | 在 WSL 里跑上面那个自检 |

## 改代码后怎么重新部署

```powershell
# 1. 本机编译（32 核，几秒）
wsl -d ts3build -e bash -c "bash /mnt/c/Users/anzu/Documents/xin/ts3web/build-wsl.sh"

# 2. 推到 NAS 并重启（NAS 只打包，不编译）
wsl -d ts3build -e bash -c "bash /mnt/c/Users/anzu/Documents/xin/ts3web/deploy/deploy-from-pc.sh"
```

## 验证手段

```bash
# 后端活着吗（NAS 上）
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18080/healthz

# TS3 侧：连上、进频道、发 10 秒 440Hz、并统计收到的语音帧
cd /opt/ts3web && docker compose run --rm -e TS3WEB_SELFTEST=10 ts3web

# 整条 WebSocket 链路（不用浏览器）
wsl -d ts3build -e bash -c "bash /mnt/c/Users/anzu/Documents/xin/ts3web/deploy/run-ws-test.sh 10"
```

## 排查

| 现象 | 原因/处理 |
|---|---|
| 页面能打开，点加入报 WebSocket 错 | 反代问题：`docker exec 1Panel-openresty-CM7c nginx -t`，看 `zz-https-ports.conf` 里的 18080 段 |
| 提示"访问口令不正确" | URL 里 `?t=` 与 `/opt/ts3web/.env` 的 `TS3WEB_TOKEN` 不一致 |
| 别人听不见网页用户 | 访客身份 talk power 不足：给 Guest 组加 `i_client_talk_power`，或发 privilege key |
| 网页用户听不见别人 | 频道 `i_channel_needed_subscribe_power` 高于访客权限；或对方不在同一频道 |
| 页面被截断/空白 | 服务端 HTTP 层在关闭连接前没把请求读干净（会触发 RST）——`src/http.rs` 里已有处理，别删 |
| 想抓包调试 | 容器只绑了本机：`ssh -L 18080:127.0.0.1:18080 root@10.0.0.123` 后直连 `ws://127.0.0.1:18080/ws` |

## 已知限制

- 浏览器需支持 WebCodecs 的 `AudioEncoder`：**Chrome / Edge 可以**，Firefox / Safari 支持不全。
- 走 TCP，弱网（丢包抖动大的手机网络）下会有偶发卡顿；这是换取"不用动路由器"的代价。
- 网页用户是未注册访客，退出后 TS3 里的身份就没了（没有持久权限）。

## 功能与踩坑记录

### 网页端能看到/切换频道（v0.2）

- 服务端在连接后、每次 book 事件、以及每 3 秒兜底，向浏览器下发 `{"type":"state"}`：
  `you`（自己是谁/在哪个频道）+ `channels`（频道树 + 每个频道的成员名单）。
- 浏览器点频道即切换；"谁在说话"由收到的语音帧里的 speaker_id 推导，450ms 后自动消失。
- 名单**必须等 book 填充完**才能构造：刚连上时 `clients/channels` 还是空的，
  所以连接后有最多 2.5 秒的等待循环，另外每 3 秒兜底重发（同时充当 WebSocket 保活）。

### 界面风格：直接复用导航页的样式表

不是"照感觉仿"，而是**把导航页的 `css/style.css` 整份内联进来**，结构也用它的类名：

| 复用自导航页 | 用在语音页的哪里 |
|---|---|
| `.stage / .frame / .frame-glow / .tape / .orn` | 大外框 + 胶带 + 四角纹样 |
| `.frame-grid / .side / .divider / .main` | 左（logo+立绘）/ 竖虚线 / 右（语音控制） |
| `.brand / .brand-art / .brand-img / .brand-sub` | 顶部 logo + 「✿ 许雪然的 TS3 语音频道 ✿」 |
| `.char / .char-img / .char-shadow / .char-spark` | 立绘（`xueran-desk.png`）与闪光 |
| `.bubble` | 状态提示会从立绘旁边冒出来 |
| `.btn-main / .btn-ghost` | 加入语音 / 离开 / 静音 |
| `.hello / .hello-time` | 右上角时钟 |
| `.tiles-head / .tiles-title / .tiles-tip` | 「频道 / 成员」标题行 |
| `--pink-* / --deep / --gold / --font / --r-frame` | 全部配色、字体、圆角、光标（连粉色爱心鼠标都有） |

只在导航页样式之上补了三件事（`web/_overrides.css`）：

1. **允许滚动** —— 导航页写死了 `body{overflow:hidden}` + `.stage{position:fixed}`（满屏仪表盘），
   语音页是内容页，必须改成正常流式布局；
2. 表单控件（圆角输入框、滑块、`<meter>`）、二级菜单 `<details>`、频道/成员列表；
3. 窄屏适配（≤760px 时收成单栏并隐藏立绘）。

改皮 / 同步导航页样式：

```bash
# 导航页样式更新后，重新拷一份（这两条在 Windows 上跑）
scp -i <key> root@10.0.0.123:/opt/1panel/www/sites/inory.xin/index/css/style.css ts3web/web/_nav-style.css
# 改 web/_overrides.css（补丁样式）或 web/_body.html（结构）后重新装配
python3 web/_reskin.py          # 会在 WSL 里跑；逻辑原样保留，只换 head/body
bash build-wsl.sh && bash deploy/deploy-from-pc.sh
```

### 麦克风发送方式（网页端，默认语音感应激活）

| 方式 | 行为 | 说明 |
|---|---|---|
| **语音感应激活（默认）** | 麦克风电平超过阈值才发，阈值默认 **-30 dB**，并保持 400ms 不切断（防止切掉尾音） | 静音时不占流量，也没有底噪；阈值可在 -60 ~ -5 dB 之间拖 |
| 持续通话 | 一直按 20ms 一帧发送 | 流量最大，但最不容易漏字 |
| 按键通话 | 按住 **空格键** 才发送 | 在输入框里打字时不会误触发 |

页面上会实时显示：`麦克风电平 -42 dB（阈值 -30 dB）　🔇 安静中，不发送`，
电平表上的刻度就是阈值位置，边说话边拖滑块就能调到合适。

实现说明：判据是采集块（960 样本）的 RMS 换算成 dBFS，做的是"电平门限 + 保持时间"。
浏览器给到网页的是**已经过 AEC/降噪处理**的信号，所以这里不需要 TS3 那种
"Hybrid（前/后处理信号混合判断）"的额外逻辑——效果上等价，但实现更简单。

### 音频处理与单人音量（网页端）

页面上有一个「音频处理」面板，**默认全部开启**：

| 开关 | 作用 | 实现方式 |
|---|---|---|
| 回声消除 | 防止自己听到自己 | `getUserMedia({echoCancellation:true})` |
| 噪声抑制（降噪） | 压掉键盘声/空调声等稳态噪声 | `getUserMedia({noiseSuppression:true})` |
| 麦克风自动增益 | 声音小的人也能被听清 | `getUserMedia({autoGainControl:true})` |
| 自动平衡各人音量 | 有人声音大有人声音小 → 自动压低大的、抬高小的 | WebAudio `DynamicsCompressor`（阈值 -30dB、6:1） |

前三个改了会**立刻重新取一次麦克风**生效（`stopCapture()` + `startCapture()`）。

**每个人的单独音量**：频道成员名字旁边有个小滑块（0–200%），按昵称存在 `localStorage`
（`ts3web_uvol_<昵称>`），下次进来还记得。播放链路是：

```
每个人自己的 GainNode -> 总音量 GainNode -> 自动平衡(压缩器) -> 输出
```

TS3 客户端的「打字声音衰减」「回声减少（播放侧）」在浏览器里没有对应能力，
浏览器侧的回声控制是在**采集侧**做的（WebRTC 的 AEC），效果通常比 TS3 的更好。

### 频道顺序要顺着"链表"走（实测）

TS3 的 `channel_order` **不是可排序的序号**，而是链表指针：它的值表示"排在我前面的那个频道 id"，
`0` 表示"我是这一层的第一个"。所以按数值排序会得到错误顺序（分隔频道会跑到最后），
必须从 `order == 0` 的那个开始、顺着"谁的 order 等于我的 id"一路走下去，
得到的顺序才和 TS3 客户端里看到的一致。

### 名单要"订阅频道"才完整（实测）

TS3 只会把**已订阅频道**的成员信息发给客户端。不订阅的话，名单里只能看到自己所在频道的人，
其它频道一律显示 0 人（哪怕那里明明有人）。

所以连上后要发一条 `channelsubscribeall`（真正的 TS3 客户端也会这么做）。
同一个坑的另一个表现：只 `clientmove cid=...` 进某个频道后，才刚好能看到那个频道的人。

### clientmove 必须同时给 clid 和 cid（实测）

```
clientmove cid=1              -> ParameterNotFound   ✗
clientmove cid=1 cpw=         -> ParameterNotFound   ✗
clientmove clid=140 cid=1     -> OK                  ✓
```

只给 `cid` 时 TS3 认为参数缺失；`clid` 是要移动的客户端 id（自己）。
用 `deploy/raw-probe.sh` 可以复现这个对照实验。

### 断开要"优雅"

WebSocket 一关就丢掉连接对象的话，`clientdisconnect` 命令还没发出去，
TS3 只能等 ping 超时才清理，日志里会留下「xxx 超时了 (connection lost)」。
现在断开后会继续轮询约 1.5 秒，让命令真正发出去。

### 排查用的调试开关

| 环境变量 | 作用 |
|---|---|
| `TS3WEB_LOG=debug` | 详细日志（含收发的 TS3 命令） |
| `TS3WEB_ALLOW_RAW=1` | 允许网页发 `{"type":"raw","cmd":"whoami"}` 这类原始 TS3 命令，用于定位命令格式（默认关闭） |
| `TS3WEB_SELFTEST=<秒>` | 自检模式：连 TS3、进频道、发测试音、统计收到多少语音帧 |

配套脚本：`deploy/raw-probe.sh`（命令探针）、`deploy/debug-move.sh`（切频道调试）、
`deploy/run-ws-test.sh`（整条 WebSocket 链路自检，可选切频道）。
