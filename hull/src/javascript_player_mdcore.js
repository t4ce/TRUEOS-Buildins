// The disposable media inspector needs MDCore's source configuration only.
// Evaluate the page's own assignments (including packed JavaScript), then use
// the existing video URL checks/handoff without loading the browser player UI.
(() => {
    const core = Object.create(null);
    let source = '';
    Object.defineProperty(core, 'wurl', {
        enumerable: true,
        get() { return source; },
        set(value) {
            source = String(value);
            if (!source) return;
            const video = document.querySelector('video');
            if (!video) return;
            video.src = source;
            video.play().catch(error => console.warn('hull: native video handoff: '+String(error)));
        }
    });
    globalThis.MDCore = core;
})();
