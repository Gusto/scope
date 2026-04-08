import {themes as prismThemes} from 'prism-react-renderer';
import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';

const config: Config = {
  title: 'Scope',
  tagline: 'Scope your machine',
  // favicon: 'img/favicon.ico',

  // Set the production url of your site here
  url: 'https://docs.gusto.com',
  // Set the /<baseUrl>/ pathname under which your site is served
  // For GitHub pages deployment, it is often '/<projectName>/'
  baseUrl: '/scope',

  // GitHub pages deployment config.
  // If you aren't using GitHub pages, you don't need these.
  organizationName: 'Gusto', // Usually your GitHub org/user name.
  projectName: 'scope', // Usually your repo name.

  onBrokenLinks: 'throw',
  onBrokenMarkdownLinks: 'warn',
  trailingSlash: false,

  staticDirectories: ['static'],

  // Even if you don't use internationalization, you can use this field to set
  // useful metadata like html lang. For example, if your site is Chinese, you
  // may want to replace "en" with "zh-Hans".
  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: './sidebars.ts',
          // Please change this to your repo.
          // Remove this to remove the "edit this page" links.
          editUrl:
            'https://github.com/Gusto/scope/tree/main/docs/',
        },
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],
  // Allows use of @theme/JSONSchemaEditor or @theme/JSONSchemaViewer
  themes: ["docusaurus-json-schema-plugin"],

  themeConfig: {
    colorMode: {
      defaultMode: 'dark',
    },
    // Replace with your project's social card
    // image: 'img/docusaurus-social-card.jpg',
    navbar: {
      title: 'Scope',
      logo: {
        alt: 'Scope',
        src: 'img/logo.svg',
      },
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'tutorialSidebar',
          position: 'left',
          label: 'Docs',
        },
        {
          href: 'https://github.com/Gusto/scope',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Docs',
          items: [
            {
              label: 'Tutorial',
              to: '/docs/intro',
            },
          ],
        },
        {
          title: 'More',
          items: [
            {
              label: 'GitHub',
              href: 'https://github.com/Gusto/scope',
            },
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} My Project, Inc. Built with Docusaurus.`,
    },
    prism: {
      theme: prismThemes.github,
      darkTheme: prismThemes.dracula,
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
