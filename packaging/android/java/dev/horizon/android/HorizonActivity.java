package dev.horizon.android;

/** SAF result callback and the voice chat microphone permission. The native activity, UI and file operations stay Rust. */
public final class HorizonActivity extends android.app.NativeActivity {
    static { System.loadLibrary("horizon_android"); }
    private native void nativeDocumentResult(int request, int result, String uri);

    @Override protected void onActivityResult(int request, int result, android.content.Intent data) {
        super.onActivityResult(request, result, data);
        if (request != 4101) return;
        nativeDocumentResult(request, result,
                data == null || data.getData() == null ? null : data.getData().toString());
    }

    @Override protected void onStart() {
        super.onStart();
        // Simple Voice Chat needs the microphone; ask once, early, so the
        // first voice session does not open a dead input stream.
        if (checkSelfPermission(android.Manifest.permission.RECORD_AUDIO)
                != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] { android.Manifest.permission.RECORD_AUDIO }, 4102);
        }
    }
}
