
// ================= ts3web：在导航页风格之上补的小东西 =================
// 1) 时钟（导航页同款样式 .hello-time）
(function clock() {
  const t = document.getElementById("clockTime");
  const s = document.getElementById("clockSec");
  if (!t || !s) return;
  const tick = () => {
    const d = new Date();
    t.textContent = String(d.getHours()).padStart(2, "0") + ":" + String(d.getMinutes()).padStart(2, "0");
    s.textContent = String(d.getSeconds()).padStart(2, "0");
  };
  tick();
  setInterval(tick, 1000);
})();

// 2) 立绘气泡：状态变化时冒一句话（导航页同款 .bubble）
(function bubbleHook() {
  const b = document.getElementById("bubble");
  if (!b) return;
  const original = b.textContent;
  let timer = null;
  window.__sayBubble = (text) => {
    b.textContent = text;
    b.classList.add("show");
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => { b.classList.remove("show"); b.textContent = original; }, 4200);
  };
})();

// 4) 状态变化时，让立绘冒个气泡说话（用导航页的 .bubble 动画）
(function statusHook() {
  if (typeof setStatus !== "function" || setStatus.__wrapped) return;
  const orig = setStatus;
  const wrapped = function (text, kind) {
    orig(text, kind);
    if (window.__sayBubble && typeof text === "string" && text.trim()) {
      // 完整显示（气泡高度有限，靠 CSS 的 max-height 兜底，不再截断文字）
      window.__sayBubble(text.trim());
    }
  };
  wrapped.__wrapped = true;
  window.setStatus = wrapped;
  try { setStatus = wrapped; } catch (e) {}
})();

// 5) 樱花飘落（纯 CSS 动画的关键帧；导航页用 canvas 画，这里用 CSS 更省）
(function petals() {
  if (document.getElementById("ts3web-petal-style")) return;
  const st = document.createElement("style");
  st.id = "ts3web-petal-style";
  st.textContent = ".petal{position:absolute;top:-32px;color:#ffb3d3;opacity:.75;animation-name:ts3fall;animation-timing-function:linear;animation-iteration-count:infinite}" +
    "@keyframes ts3fall{to{transform:translateY(112vh) rotate(340deg)}}";
  document.head.appendChild(st);
})();

// 6) 看板娘：点一下换一张 Q 版图 + 随机台词（沿用导航页的 .pet / .pet-bubble）
(function pet() {
  const CHIBI = ["q1", "q2", "q3", "q5", "q6", "q7", "q8", "q10", "yaoshikou"]
    .map((n) => "assets/chibi/chibi-" + n + ".png");   // 相对路径：由本服务代发，内网外网都能出图
  const PET_LINES = ["喵～", "点我做什么呀？", "要好好休息哦！", "今天想我了没？",
    "呜哇，被发现了！", "陪你一起上网呀～", "嘿嘿，摸摸头 ✿"];
  const el = document.getElementById("pet");
  const bubble = document.getElementById("petBubble");
  const img = document.getElementById("petImg");
  if (!el || !bubble || !img) return;

  // 预加载，点的时候秒切
  CHIBI.slice(1).forEach((src) => { const i = new Image(); i.src = src; });

  let idx = 0, timer = null;
  const wiggle = () => {
    idx = (idx + 1) % CHIBI.length;
    img.src = CHIBI[idx];
    bubble.textContent = PET_LINES[Math.floor(Math.random() * PET_LINES.length)];
    bubble.classList.add("show");
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => bubble.classList.remove("show"), 3000);
  };
  el.addEventListener("click", wiggle);
  el.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); wiggle(); }
  });
})();

// 7) 暗色模式：浅色「樱花」/ 暗色「夜樱」（沿用导航页的配色与 theme-btn）
(function theme() {
  const PRESETS = {
    light: { grad: "linear-gradient(150deg,#ffd3e5,#fff1f7 45%,#eaf2ff)", bright: 100, dim: 0.12, meta: "#ffd9e8" },
    dark:  { grad: "linear-gradient(150deg,#2a1a26,#3a2233 45%,#241a33)", bright: 108, dim: 0.45, meta: "#241722" },
  };
  const KEY = "ts3web_theme";
  const get = () => { try { return localStorage.getItem(KEY) === "dark" ? "dark" : "light"; } catch (e) { return "light"; } };

  function applyTheme() {
    const t = get();
    const p = PRESETS[t];
    document.documentElement.dataset.theme = t;       // 触发导航页样式里的 html[data-theme="dark"] 覆盖
    const bg = document.getElementById("bg");
    if (bg) {
      bg.style.backgroundImage = p.grad;
      bg.style.filter = "brightness(" + (p.bright / 100) + ") saturate(1.08)";
    }
    document.documentElement.style.setProperty("--bg-dim", String(p.dim));
    const meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute("content", p.meta);
    const btn = document.getElementById("btnTheme");
    if (btn) btn.title = t === "dark" ? "现在是夜樱（暗色），点一下变回樱花（D）" : "现在是樱花（浅色），点一下进夜樱（D）";
  }

  function toggleTheme() {
    try { localStorage.setItem(KEY, get() === "dark" ? "light" : "dark"); } catch (e) {}
    applyTheme();
  }

  applyTheme();
  const btn = document.getElementById("btnTheme");
  if (btn) btn.addEventListener("click", toggleTheme);
  window.addEventListener("keydown", (ev) => {
    if (ev.key !== "d" && ev.key !== "D") return;
    const t = ev.target;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;   // 打字时不抢键
    toggleTheme();
  });
})();
