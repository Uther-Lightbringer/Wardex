// 界面风格注册表（docs/ui-design.md §8.4）：war = 魔兽贴图风（默认），
// pure = 纯净风（纯 CSS 渲染路径，浅色半透明玻璃 + 常规紧凑按钮）。
// 加新主题 = 在 THEMES 里加一项 + 可选 CSS 覆盖，不动组件库。
//
// 决策（协作动态 2026-08）：pure 模式默认纯白底，但仍显示用户自定义
// 上传的背景视频/图片；按钮换常规紧凑（高 ~32px）；第一期主流程（配置页/
// Hub/会话列表/聊天）走 plain 路径，战场监控页先给浅色半透明 CSS 兜底。

export type ThemeKind = 'art' | 'plain';
export type ThemeFont = 'serif' | 'sans';

/** 设置页「个性」组中该风格专属的设置项 id（共性项始终显示，见 SettingsPage）。 */
export type ThemeSettingId = 'chatAlpha' | 'pageColor' | 'bgBrightness';

export interface ThemeDef {
  id: string;
  label: string;
  kind: ThemeKind;
  /** 是否渲染左右常驻铁轨（war=58px 装饰条）。 */
  rails: boolean;
  /** 是否用 WC3 剑形光标。 */
  cursor: boolean;
  font: ThemeFont;
  /** 无自定义背景时的默认底：war=内置视频；pure=纯白。 */
  defaultBg: 'video' | 'white';
  /** 风格专属设置项（设置页「个性」组动态渲染；war 暂无 → 仅共性）。 */
  settings: ThemeSettingId[];
}

export const THEMES: ThemeDef[] = [
  {
    id: 'war',
    label: '魔兽风格',
    kind: 'art',
    rails: true,
    cursor: true,
    font: 'serif',
    defaultBg: 'video',
    settings: [],
  },
  {
    id: 'pure',
    label: '纯净风格',
    kind: 'plain',
    rails: false,
    cursor: false,
    font: 'sans',
    defaultBg: 'white',
    settings: ['chatAlpha', 'pageColor', 'bgBrightness'],
  },
];

export const DEFAULT_THEME = 'war';
export const THEME_IDS: string[] = THEMES.map((t) => t.id);

export function themeOf(id: string | null | undefined): ThemeDef {
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}
