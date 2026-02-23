import daisyui from 'daisyui';

export default {
  content: ['./index.html', './src/**/*.{js,jsx}'],
  theme: {},
  plugins: [daisyui],
  daisyui: {
    themes: ['light', 'dark'],
  },
};
