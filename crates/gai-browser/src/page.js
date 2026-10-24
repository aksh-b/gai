// Functions gai evaluates in the page. Each top-level entry is a function
// expression; Rust calls them as `(fn)(args)`.
//
// outline(maxChars): the page as short lines, in reading order. Headings
// start with #, text is plain, and every element the agent can act on gets a
// number, kept on the element as data-gai-ref so later calls can find it.
const outline = (maxChars) => {
  const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'SVG', 'IFRAME', 'OBJECT', 'CANVAS', 'HEAD']);
  const BLOCK = new Set(['P', 'DIV', 'LI', 'TR', 'TD', 'TH', 'DD', 'DT', 'SECTION', 'ARTICLE', 'HEADER', 'FOOTER', 'NAV', 'MAIN', 'ASIDE', 'BLOCKQUOTE', 'PRE', 'FIGCAPTION', 'TABLE', 'UL', 'OL', 'FORM', 'FIELDSET', 'BR', 'HR', 'LABEL']);
  const INTERACTIVE = 'a[href],button,input:not([type=hidden]),select,textarea,summary,[role=button],[role=link],[role=checkbox],[role=radio],[role=tab],[role=menuitem],[role=switch],[role=combobox],[contenteditable=""],[contenteditable=true]';
  const clean = (s) => (s || '').replace(/\s+/g, ' ').trim();
  const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + '…' : s);
  const visible = (el) => (el.checkVisibility ? el.checkVisibility({ visibilityProperty: true }) : el.offsetParent !== null || el.tagName === 'BODY');
  const label = (el) => {
    const byId = el.id && document.querySelector(`label[for="${CSS.escape(el.id)}"]`);
    return clean(
      el.getAttribute('aria-label') ||
        (el.labels && el.labels[0] && el.labels[0].innerText) ||
        (byId && byId.innerText) ||
        el.getAttribute('placeholder') ||
        el.getAttribute('title') ||
        el.getAttribute('alt') ||
        (el.tagName === 'INPUT' ? '' : el.innerText) ||
        el.getAttribute('name') ||
        '',
    );
  };
  const submits = (el) => {
    const t = (el.getAttribute('type') || '').toLowerCase();
    if (el.tagName === 'BUTTON') return !!el.form && (t === '' || t === 'submit');
    if (el.tagName === 'INPUT') return !!el.form && (t === 'submit' || t === 'image');
    return false;
  };
  const describe = (el, n) => {
    const tag = el.tagName;
    const role = el.getAttribute('role');
    const name = clip(label(el), 80);
    const q = name ? ` "${name}"` : '';
    if (tag === 'A') {
      const href = el.getAttribute('href') || '';
      return `[${n}] link${q} -> ${clip(href, 100)}`;
    }
    if (tag === 'SELECT') {
      const opts = [...el.options].map((o) => clean(o.text)).filter(Boolean);
      const cur = el.selectedOptions[0] ? clean(el.selectedOptions[0].text) : '';
      return `[${n}] select${q} = "${clip(cur, 60)}" options: ${clip(opts.slice(0, 12).join(' | '), 300)}${opts.length > 12 ? ` (+${opts.length - 12})` : ''}`;
    }
    if (tag === 'INPUT' || tag === 'TEXTAREA') {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      if (t === 'checkbox' || t === 'radio') return `[${n}] ${t}${q}${el.checked ? ' (checked)' : ''}`;
      if (t === 'submit' || t === 'button' || t === 'reset' || t === 'image') {
        return `[${n}] button "${clip(clean(el.value) || name, 80)}"${submits(el) ? ' (submits a form)' : ''}`;
      }
      const v = t === 'password' ? (el.value ? '••••' : '') : clean(el.value);
      return `[${n}] ${t === 'text' || tag === 'TEXTAREA' ? 'textbox' : t + ' field'}${q}${v ? ` = "${clip(v, 60)}"` : ''}${el.disabled ? ' (disabled)' : ''}`;
    }
    if (tag === 'BUTTON') return `[${n}] button${q}${submits(el) ? ' (submits a form)' : ''}${el.disabled ? ' (disabled)' : ''}`;
    return `[${n}] ${role || (tag === 'SUMMARY' ? 'toggle' : 'editable')}${q}`;
  };

  document.querySelectorAll('[data-gai-ref]').forEach((e) => e.removeAttribute('data-gai-ref'));
  const lines = [];
  let text = '';
  let n = 0;
  let size = 0;
  let more = 0;
  const push = (line) => {
    if (!line) return;
    if (size + line.length > maxChars) {
      more++;
      return;
    }
    lines.push(line);
    size += line.length + 1;
  };
  const flush = () => {
    const t = clean(text);
    text = '';
    if (t) push(clip(t, 400));
  };
  const walk = (node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      text += ' ' + node.nodeValue;
      return;
    }
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    const el = node;
    if (SKIP.has(el.tagName.toUpperCase()) || !visible(el)) return;
    if (el.matches(INTERACTIVE)) {
      flush();
      n++;
      el.setAttribute('data-gai-ref', String(n));
      push(describe(el, n));
      return;
    }
    if (/^H[1-6]$/.test(el.tagName)) {
      flush();
      const t = clean(el.innerText);
      if (t) push('#'.repeat(Number(el.tagName[1])) + ' ' + clip(t, 200));
      return;
    }
    if (el.tagName === 'IMG') {
      const alt = clean(el.getAttribute('alt'));
      if (alt) text += ` [image: ${clip(alt, 80)}]`;
      return;
    }
    const block = BLOCK.has(el.tagName);
    if (block) flush();
    for (const c of el.childNodes) walk(c);
    if (el.shadowRoot) for (const c of el.shadowRoot.childNodes) walk(c);
    if (block) flush();
  };
  if (document.body) walk(document.body);
  flush();
  if (more) lines.push(`… ${more} more lines not shown (raise max_chars, or use browser_extract)`);
  return { title: document.title, url: location.href, text: lines.join('\n') };
};

// text(maxChars): the readable text of the main content.
const text = (maxChars) => {
  const root = document.querySelector('main, article, [role=main]') || document.body;
  let t = (root ? root.innerText : '').replace(/\n{3,}/g, '\n\n').trim();
  const cut = t.length > maxChars;
  if (cut) t = t.slice(0, maxChars) + '\n… (truncated)';
  return { title: document.title, url: location.href, text: t };
};

// extract([selector, attributes, limit]): matching elements as objects.
const extract = ([selector, attributes, limit]) => {
  const els = [...document.querySelectorAll(selector)];
  return {
    total: els.length,
    items: els.slice(0, limit).map((el) => {
      const o = { text: (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 1000) };
      for (const a of attributes) {
        // Properties give absolute URLs for href/src.
        const v = a === 'href' || a === 'src' ? el[a] || el.getAttribute(a) : a === 'value' ? el.value : el.getAttribute(a);
        if (v != null && v !== '') o[a] = String(v);
      }
      return o;
    }),
  };
};
