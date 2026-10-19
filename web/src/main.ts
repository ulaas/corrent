import 'virtual:tokens.css';
import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { initI18n } from './lib/i18n.svelte';
import { applyPrefs } from './lib/prefs.svelte';

applyPrefs();
await initI18n();
mount(App, { target: document.getElementById('app')! });
