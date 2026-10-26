import { defineConfig } from 'vocs'
import { sections, repository } from './content.mjs'

export default defineConfig({
  title: 'Tile-Ripper',
  description: 'Inspect acquisition plans and retrieve native scientific raster subsets in Rust.',
  iconUrl: '/favicon.svg',
  aiCta: false,
  theme: {
    accentColor: { light: '#0a6b78', dark: '#5bbfca' },
    variables: {
      fontFamily: {
        default: 'Optima, Candara, "Trebuchet MS", sans-serif',
        mono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      },
    },
  },
  topNav: [
    { text: 'Quickstart', link: '/quickstart' },
    { text: 'Installation', link: '/installation' },
    { text: 'Supported data', link: '/supported-data' },
    { text: 'ChronoZarr', link: 'https://chronozarr.org' },
  ],
  socials: [{ icon: 'github', link: repository }],
  sidebar: sections.map(section => ({ text: section.text,
    collapsed: section.collapsed,
    items: section.pages.map(([, route, text]) => ({ text, link: '/' + route })),
  })),
})
