use hull_quickjs::Engine;

fn verify(source: &str) {
    let mut engine = Engine::new().unwrap();
    engine
        .eval(
            include_bytes!("../../../src/javascript_helpers.js"),
            "hull:helpers",
            false,
        )
        .unwrap();
    engine
        .eval(source.as_bytes(), "helpers-test.js", false)
        .unwrap();
}

#[test]
fn base64_matches_known_vectors_and_round_trips_every_byte() {
    verify(
        r#"
        for (const [text, encoded] of [['',''], ['f','Zg=='], ['fo','Zm8='], ['foo','Zm9v'],
                ['foob','Zm9vYg=='], ['fooba','Zm9vYmE='], ['foobar','Zm9vYmFy'], ['\0\xff','AP8=']]) {
            if (btoa(text) !== encoded || atob(encoded) !== text) throw Error('known vector failed');
        }
        let bytes = '';
        for (let byte = 0; byte < 256; byte++) bytes += String.fromCharCode(byte);
        if (atob(btoa(bytes)) !== bytes) throw Error('all-byte round trip failed');
    "#,
    );
}

#[test]
fn base64_decoder_accepts_ascii_whitespace_and_optional_padding() {
    verify(
        r#"
        for (const encoded of ['Zg', 'Zg==', 'Z h = =\t\n\f\r ', 'Zh']) {
            if (atob(encoded) !== 'f') throw Error('forgiving single-byte decode failed');
        }
        for (const encoded of ['Zm8', 'Zm8=', 'Zm9']) {
            if (atob(encoded) !== 'fo') throw Error('forgiving double-byte decode failed');
        }
        if (atob('\t\n\f\r ') !== '') throw Error('whitespace-only input failed');
    "#,
    );
}

#[test]
fn base64_rejects_bad_padding_alphabet_lengths_and_non_byte_strings() {
    verify(
        r#"
        for (const encoded of ['A', 'AAAAA', '=', '==', '===', '====', 'Zg=', 'Zg===',
                'Zm8==', 'Z=g=', 'Zg==A', 'Zg==\v', 'Zg==\u00a0', 'Zm_8', 'Zm-8', '😀']) {
            let rejected = false;
            try { atob(encoded); } catch (error) { rejected = error.name === 'InvalidCharacterError'; }
            if (!rejected) throw Error('invalid Base64 accepted: ' + encoded);
        }
        for (const text of ['\u0100', '\ud800', '\udc00', '😀', 'ab\u0100', 'abc\u0100']) {
            let rejected = false;
            try { btoa(text); } catch (error) { rejected = error.name === 'InvalidCharacterError'; }
            if (!rejected) throw Error('non-byte string accepted');
        }
    "#,
    );
}

#[test]
fn base64_uses_string_coercion_and_requires_an_argument() {
    verify(
        r#"
        if (btoa(null) !== 'bnVsbA==' || btoa(undefined) !== 'dW5kZWZpbmVk' || btoa(123) !== 'MTIz')
            throw Error('primitive coercion failed');
        if (btoa({toString() { return 'f'; }}) !== 'Zg==' || atob({toString() { return 'Zg'; }}) !== 'f')
            throw Error('object coercion failed');
        for (const call of [() => atob(), () => btoa(), () => atob(Symbol()), () => btoa(Symbol())]) {
            let rejected = false;
            try { call(); } catch (error) { rejected = error instanceof TypeError; }
            if (!rejected) throw Error('missing argument or Symbol accepted');
        }
        if (typeof document !== 'undefined' || typeof DOMException !== 'undefined')
            throw Error('helpers introduced browser objects');
    "#,
    );
}
