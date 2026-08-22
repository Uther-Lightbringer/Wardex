// env-proxy.mjs — 临时注入：让全局 fetch (undici) 走 HTTP_PROXY/HTTPS_PROXY
// 通过 NODE_OPTIONS="--import=file://<abs path>/env-proxy.mjs" 启用。
// undici 装在 pi workspace 根 node_modules；用绝对路径避免 import 解析失败。
import { setGlobalDispatcher, EnvHttpProxyAgent } from "file:///C:/workspace/pi/node_modules/undici/index.js";

const proxy = process.env.HTTPS_PROXY || process.env.HTTP_PROXY || process.env.https_proxy;
if (proxy) {
  setGlobalDispatcher(new EnvHttpProxyAgent());
  console.log(`[env-proxy] 全局 fetch 已走代理: ${proxy}`);
} else {
  console.warn("[env-proxy] 未检测到 HTTPS_PROXY/HTTP_PROXY，未启用代理。");
}
