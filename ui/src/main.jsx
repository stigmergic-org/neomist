import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App.jsx';
import './index.css';

const themeMedia = window.matchMedia('(prefers-color-scheme: dark)');

const setFavicon = (useDark) => {
  const href = useDark ? '/icon-dark.svg' : '/icon.svg';
  let link = document.querySelector('link[rel="icon"]');
  if (!link) {
    link = document.createElement('link');
    link.rel = 'icon';
    link.type = 'image/svg+xml';
    document.head.appendChild(link);
  }
  link.href = href;
};

const applyTheme = (matches) => {
  const useDark = typeof matches === 'boolean' ? matches : themeMedia.matches;
  const theme = useDark ? 'neomist_vapor_dark' : 'neomist_vapor';
  document.documentElement.setAttribute('data-theme', theme);
  document.documentElement.style.colorScheme = useDark ? 'dark' : 'light';
  setFavicon(useDark);
};

applyTheme();

if (themeMedia.addEventListener) {
  themeMedia.addEventListener('change', (event) => applyTheme(event.matches));
} else if (themeMedia.addListener) {
  themeMedia.addListener((event) => applyTheme(event.matches));
}

ReactDOM.createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
