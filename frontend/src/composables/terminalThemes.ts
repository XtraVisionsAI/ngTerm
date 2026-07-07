import type { ITheme } from '@xterm/xterm'

export interface AppThemeColors {
  bg: string
  bgPanel: string
  bgHover: string
  bgInput: string
  border: string
  text: string
  textMuted: string
  textDimmed: string
  primary: string
  primaryHover: string
  onPrimary: string
  danger: string
  dangerHover: string
  success: string
  warning: string
  accent: string
  shadow: string
  scrollThumb: string
}

export interface AppTheme {
  id: string
  name: string
  colors: AppThemeColors
  terminal: ITheme
}

const githubDark: AppTheme = {
  id: 'github-dark',
  name: 'GitHub Dark',
  colors: {
    bg: '#0d1117',
    bgPanel: '#161b22',
    bgHover: '#1c2128',
    bgInput: '#0d1117',
    border: '#30363d',
    text: '#c9d1d9',
    textMuted: '#8b949e',
    textDimmed: '#6e7681',
    primary: '#3b82f6',
    primaryHover: '#2563eb',
    onPrimary: '#ffffff',
    danger: '#ff7b72',
    dangerHover: '#f85149',
    success: '#3fb950',
    warning: '#d29922',
    accent: '#58a6ff',
    shadow: 'rgb(1 4 9 / 0.45)',
    scrollThumb: '#30363d'
  },
  terminal: {
    background: '#0d1117',
    foreground: '#c9d1d9',
    cursor: '#c9d1d9',
    selectionBackground: '#264f78',
    black: '#484f58',
    red: '#ff7b72',
    green: '#3fb950',
    yellow: '#d29922',
    blue: '#58a6ff',
    magenta: '#bc8cff',
    cyan: '#39c5cf',
    white: '#b1bac4',
    brightBlack: '#6e7681',
    brightRed: '#ffa198',
    brightGreen: '#56d364',
    brightYellow: '#e3b341',
    brightBlue: '#79c0ff',
    brightMagenta: '#d2a8ff',
    brightCyan: '#56d4dd',
    brightWhite: '#f0f6fc'
  }
}

const nord: AppTheme = {
  id: 'nord',
  name: 'Nord',
  colors: {
    bg: '#2e3440',
    bgPanel: '#3b4252',
    bgHover: '#434c5e',
    bgInput: '#2e3440',
    border: '#4c566a',
    text: '#d8dee9',
    textMuted: '#81a1c1',
    textDimmed: '#7b88a1',
    primary: '#88c0d0',
    primaryHover: '#7bb8c9',
    onPrimary: '#2e3440',
    danger: '#bf616a',
    dangerHover: '#a94f59',
    success: '#a3be8c',
    warning: '#ebcb8b',
    accent: '#5e81ac',
    shadow: 'rgb(15 17 21 / 0.36)',
    scrollThumb: '#4c566a'
  },
  terminal: {
    background: '#2e3440',
    foreground: '#d8dee9',
    cursor: '#d8dee9',
    selectionBackground: '#434c5e',
    black: '#3b4252',
    red: '#bf616a',
    green: '#a3be8c',
    yellow: '#ebcb8b',
    blue: '#81a1c1',
    magenta: '#b48ead',
    cyan: '#88c0d0',
    white: '#e5e9f0',
    brightBlack: '#4c566a',
    brightRed: '#d06f79',
    brightGreen: '#b1cc99',
    brightYellow: '#f1d59d',
    brightBlue: '#93b4d6',
    brightMagenta: '#c59cbd',
    brightCyan: '#9bd3e4',
    brightWhite: '#eceff4'
  }
}

const catppuccin: AppTheme = {
  id: 'catppuccin',
  name: 'Catppuccin',
  colors: {
    bg: '#1e1e2e',
    bgPanel: '#181825',
    bgHover: '#313244',
    bgInput: '#181825',
    border: '#45475a',
    text: '#cdd6f4',
    textMuted: '#a6adc8',
    textDimmed: '#6c7086',
    primary: '#cba6f7',
    primaryHover: '#b48bf2',
    onPrimary: '#1e1e2e',
    danger: '#f38ba8',
    dangerHover: '#e56f8e',
    success: '#a6e3a1',
    warning: '#f9e2af',
    accent: '#f5c2e7',
    shadow: 'rgb(0 0 0 / 0.4)',
    scrollThumb: '#45475a'
  },
  terminal: {
    background: '#1e1e2e',
    foreground: '#cdd6f4',
    cursor: '#f5e0dc',
    selectionBackground: '#45475a',
    black: '#45475a',
    red: '#f38ba8',
    green: '#a6e3a1',
    yellow: '#f9e2af',
    blue: '#89b4fa',
    magenta: '#f5c2e7',
    cyan: '#94e2d5',
    white: '#bac2de',
    brightBlack: '#585b70',
    brightRed: '#f5a0b7',
    brightGreen: '#b6f0b2',
    brightYellow: '#fbe8c0',
    brightBlue: '#9cc5ff',
    brightMagenta: '#f7cfed',
    brightCyan: '#a6eee2',
    brightWhite: '#cdd6f4'
  }
}

const dracula: AppTheme = {
  id: 'dracula',
  name: 'Dracula',
  colors: {
    bg: '#282a36',
    bgPanel: '#21222c',
    bgHover: '#343746',
    bgInput: '#21222c',
    border: '#44475a',
    text: '#f8f8f2',
    textMuted: '#bd93f9',
    textDimmed: '#6272a4',
    primary: '#bd93f9',
    primaryHover: '#a77bfa',
    onPrimary: '#282a36',
    danger: '#ff5555',
    dangerHover: '#ff3f3f',
    success: '#50fa7b',
    warning: '#f1fa8c',
    accent: '#ff79c6',
    shadow: 'rgb(0 0 0 / 0.4)',
    scrollThumb: '#44475a'
  },
  terminal: {
    background: '#282a36',
    foreground: '#f8f8f2',
    cursor: '#f8f8f2',
    selectionBackground: '#44475a',
    black: '#21222c',
    red: '#ff5555',
    green: '#50fa7b',
    yellow: '#f1fa8c',
    blue: '#bd93f9',
    magenta: '#ff79c6',
    cyan: '#8be9fd',
    white: '#f8f8f2',
    brightBlack: '#6272a4',
    brightRed: '#ff6e6e',
    brightGreen: '#69ff94',
    brightYellow: '#ffffa5',
    brightBlue: '#d6acff',
    brightMagenta: '#ff92df',
    brightCyan: '#a4ffff',
    brightWhite: '#ffffff'
  }
}

const tokyoNight: AppTheme = {
  id: 'tokyo-night',
  name: 'Tokyo Night',
  colors: {
    bg: '#1a1b26',
    bgPanel: '#16161e',
    bgHover: '#292e42',
    bgInput: '#16161e',
    border: '#3b4261',
    text: '#a9b1d6',
    textMuted: '#737aa2',
    textDimmed: '#565f89',
    primary: '#7aa2f7',
    primaryHover: '#6690e6',
    onPrimary: '#1a1b26',
    danger: '#f7768e',
    dangerHover: '#e9667d',
    success: '#9ece6a',
    warning: '#e0af68',
    accent: '#bb9af7',
    shadow: 'rgb(0 0 0 / 0.42)',
    scrollThumb: '#3b4261'
  },
  terminal: {
    background: '#1a1b26',
    foreground: '#a9b1d6',
    cursor: '#c0caf5',
    selectionBackground: '#33467c',
    black: '#414868',
    red: '#f7768e',
    green: '#9ece6a',
    yellow: '#e0af68',
    blue: '#7aa2f7',
    magenta: '#bb9af7',
    cyan: '#7dcfff',
    white: '#c0caf5',
    brightBlack: '#565f89',
    brightRed: '#ff8ea3',
    brightGreen: '#b9f27c',
    brightYellow: '#f4c980',
    brightBlue: '#8db8ff',
    brightMagenta: '#c7a9ff',
    brightCyan: '#9ae6ff',
    brightWhite: '#d5dcff'
  }
}

export const appThemes: AppTheme[] = [githubDark, nord, catppuccin, dracula, tokyoNight]

export const DEFAULT_THEME_ID = 'github-dark'

const STORAGE_KEY = 'onemux_theme'

export function getStoredThemeId(): string {
  const stored = localStorage.getItem(STORAGE_KEY)
  if (stored && appThemes.some((t) => t.id === stored)) return stored
  return DEFAULT_THEME_ID
}

export function setStoredThemeId(id: string) {
  localStorage.setItem(STORAGE_KEY, id)
}

export function getAppTheme(id: string): AppTheme {
  return appThemes.find((t) => t.id === id) || appThemes[0]
}
