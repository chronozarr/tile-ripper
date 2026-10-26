import { defineConfig } from 'vocs'
import { sections, repository } from './content.mjs'

export default defineConfig({
  title: 'Tile-Ripper',
  description: 'Inspect acquisition plans and retrieve native scientific raster subsets in Rust.',
  iconUrl: '/favicon.svg',
  aiCta: false,
  theme: {
    accentColor: '#0a6b78',
    variables: {
      fontFamily: {
        default: 'Optima, Candara, "Trebuchet MS", sans-serif',
        mono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      },
    },
  },
  topNav: [
    { text: 'Quickstart', link: '/quickstart' },
    { text: 'Products', link: '/products' },
    { text: 'Coverage', link: '/coverage' },
    { text: 'ChronoZarr', link: 'https://chronozarr.org' },
  ],
  socials: [{ icon: 'github', link: repository }],
  sidebar: sections.map(section => ({ text: section.text,
    items: section.pages.map(([, route, text]) => ({ text, link: '/' + route })),
  })),
})
