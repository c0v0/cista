/**
 * 解析 API base（design §4.2）。
 *
 * 同源部署（默认）返回 `""`，请求走相对路径，不需要 CORS；
 * 跨域部署返回配置的 origin，去掉尾部斜杠。代码不得假设一定分域。
 */
export function resolveApiBase(configured?: string | null): string {
  const raw = (configured ?? "").trim();
  if (raw === "" || raw === "/") return "";
  return raw.replace(/\/+$/, "");
}
