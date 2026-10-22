// Corrent website: reveal on scroll, the hero's tilt and the tour's sticky screenshots.
// Everything is visible without this script; it only adds motion.
(() => {
  const still = window.matchMedia('(prefers-reduced-motion: reduce)');

  // Reveal sections as they come into view.
  const items = document.querySelectorAll('.reveal');
  if ('IntersectionObserver' in window && !still.matches) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (e.isIntersecting) {
          e.target.classList.add('in');
          io.unobserve(e.target);
        }
      }
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.12 });
    items.forEach((el) => io.observe(el));
  } else {
    items.forEach((el) => el.classList.add('in'));
  }

  // Header border once the page scrolls; the hero screenshot straightens up as it scrolls in.
  const top = document.querySelector('.top');
  const tilt = document.querySelector('.tilt');
  let queued = false;
  const onScroll = () => {
    queued = false;
    top.classList.toggle('scrolled', window.scrollY > 8);
    if (tilt && !still.matches) {
      const r = tilt.getBoundingClientRect();
      const p = Math.min(1, Math.max(0, (window.innerHeight - r.top) / (window.innerHeight * 0.75)));
      tilt.style.setProperty('--p', p.toFixed(3));
    }
  };
  window.addEventListener('scroll', () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(onScroll);
    }
  }, { passive: true });
  window.addEventListener('resize', onScroll);
  onScroll();

  // The tour: the step in the middle of the screen picks the screenshot.
  const steps = [...document.querySelectorAll('.step')];
  const shots = [...document.querySelectorAll('.shot')];
  const dots = [...document.querySelectorAll('.dots span')];
  const show = (n) => {
    steps.forEach((s, i) => s.classList.toggle('is-on', i === n));
    shots.forEach((s, i) => s.classList.toggle('is-on', i === n));
    dots.forEach((s, i) => s.classList.toggle('is-on', i === n));
  };
  if ('IntersectionObserver' in window && steps.length) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (e.isIntersecting) show(Number(e.target.dataset.shot));
      }
    }, { rootMargin: '-45% 0px -45% 0px' });
    steps.forEach((s) => io.observe(s));
    show(0);
  }
})();
