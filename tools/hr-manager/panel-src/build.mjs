// build.mjs —— 把 panel.template.html + theme-export/dark.css + entry.js 组装成
// 单文件产物 ../src/assets/index.html：
//   * 主题 CSS 原样内联，替换模板里的 <!-- __THEME__ --> 注释占位；
//   * entry.js 用 esbuild 打包压缩（import 的 @material/web 组件全部进包），
//     替换模板里的 <!-- __BUNDLE__ --> 注释占位。
// 产物必须单文件、零外部引用：不出现 http(s) 资源与 @import，占位不允许残留。
// panel.rs 用 include_str! 吃这个文件，WebView2 走 NavigateToString（上限 2MB，
// 产物控制在 1.5MB 以内留余量），产物由 .gitignore 排除、不入库不手工编辑。
import { build } from "esbuild";
import { readFileSync, writeFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const theme = readFileSync(path.join(here, "theme-export", "dark.css"), "utf8");
const template = readFileSync(path.join(here, "panel.template.html"), "utf8");

// WebView2 是常青版 Chromium，es2022 足够；legalComments 关掉：发行包的第三方
// 声明在仓库 THIRD_PARTY_NOTICES.md，别让许可头把 http 字样带进产物。
const result = await build({
  entryPoints: [path.join(here, "entry.js")],
  bundle: true,
  minify: true,
  format: "esm",
  target: "es2022",
  legalComments: "none",
  logLevel: "info",
  write: false,
});
const js = result.outputFiles[0].text;

// 内联脚本里出现 "</script" 会提前终止标签，转义成 "<\/script"（字符串等价）。
const safeJs = js.replaceAll("</script", "<\\/script>");

// 占位替换必须用函数形式：字符串形式的替换文本会解释 $&/$'/$` 等 $ 模式，
// 压缩后的 lit 代码里就有这类字面量，会把占位符原样带进产物。
const html = template
  .replace("<!-- __THEME__ -->", () => "<style>\n" + theme + "</style>")
  .replace("<!-- __BUNDLE__ -->", () => '<script type="module">\n' + safeJs + "\n</scr" + "ipt>");

// ---- 产物自检：占位残留 / 外链 / @import 一票否决，构建直接失败
const problems = [];
if (html.includes("__THEME__") || html.includes("__BUNDLE__")) problems.push("占位符残留");
if (/(\s(?:src|href|srcset)\s*=\s*["']?\s*(?:https?:)?\/\/)|url\(\s*["']?\s*(?:https?:)?\/\//i.test(html)) {
  problems.push("出现指向网络的外部资源引用");
}
if (html.includes("@import")) problems.push("出现 @import");
if (problems.length > 0) {
  console.error("产物自检失败：" + problems.join("；"));
  process.exit(1);
}

const out = path.join(here, "..", "src", "assets", "index.html");
writeFileSync(out, html);
const size = statSync(out).size;
if (size >= 1572864) {
  console.error(`产物 ${size} 字节，超出 1.5MB 上限（NavigateToString 上限 2MB 要留余量）`);
  process.exit(1);
}
console.log(`已生成 ${out}（${size} 字节）`);
