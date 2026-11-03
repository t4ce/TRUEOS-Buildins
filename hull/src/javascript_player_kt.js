// Deliberately URL-only KVS/KT player adapter. The page supplies the same
// configuration object; browser player UI, Flash and plugins are not constructed.
globalThis.kt_player = function(_container, _asset, _width, _height, options) {
    if (!options || typeof options.video_url !== 'string')
        throw new TypeError('KT player requires video_url');
    const video = document.createElement('video');
    video.src = options.video_url;
    video.play().catch(error => console.warn('hull: native video handoff: '+String(error)));
    return video;
};
