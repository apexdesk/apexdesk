package com.apexdesk.app;

import android.Manifest;
import android.app.NativeActivity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.provider.Settings;
import android.os.Bundle;
import android.view.DisplayCutout;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

/**
 * The NativeActivity full screen (immersive): the system's bars hide (a
 * swipe from an edge shows them for a moment, over the program), so the
 * program's tab bar and search box at the top can be touched. Views are
 * only touched on this (the UI) thread, which the program's own thread
 * can't do.
 *
 * Where the system keeps bars or a notch on screen anyway (some tablets,
 * split screen, a freeform window), their size is kept here and the
 * program lays itself out inside them (crates/apex-app/src/android.rs reads
 * these fields). Hidden bars don't make the window shrink above the
 * keyboard either, so the keyboard's height is kept too.
 */
public class MainActivity extends NativeActivity {
    /** The keyboard's height in pixels, 0 while it is hidden. */
    public static volatile int keyboardHeight;
    /** What the system's bars and the display's cutout cover, in pixels. */
    public static volatile int insetLeft, insetTop, insetRight, insetBottom;

    private final Runnable hide = new Runnable() {
        @Override
        public void run() {
            hideBars();
        }
    };

    @Override
    protected void onCreate(Bundle state) {
        Window w = getWindow();
        w.addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams a = w.getAttributes();
            a.layoutInDisplayCutoutMode = Build.VERSION.SDK_INT >= 30
                    ? WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                    : WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            w.setAttributes(a);
        }
        super.onCreate(state);
        View decor = w.getDecorView();
        decor.setOnApplyWindowInsetsListener(new View.OnApplyWindowInsetsListener() {
            @Override
            public WindowInsets onApplyWindowInsets(View view, WindowInsets insets) {
                keep(insets);
                return view.onApplyWindowInsets(insets);
            }
        });
        // Older systems show the bars again after a dialog or the keyboard.
        decor.setOnSystemUiVisibilityChangeListener(new View.OnSystemUiVisibilityChangeListener() {
            @Override
            public void onSystemUiVisibilityChange(int visibility) {
                if ((visibility & View.SYSTEM_UI_FLAG_FULLSCREEN) == 0) {
                    later(1500);
                }
            }
        });
        hideBars();
        later(0);
    }

    /**
     * Whether the program may read the files of the shared storage. Without
     * it Android shows its folders but not the files in them: from Android
     * 11 "All files access" (in the settings), before it the storage
     * permission.
     */
    public boolean hasFileAccess() {
        if (Build.VERSION.SDK_INT >= 30) {
            return Environment.isExternalStorageManager();
        }
        return checkSelfPermission(Manifest.permission.READ_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED;
    }

    /** Asks for it: the settings page, or the permission's dialog. */
    public void askFileAccess() {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (Build.VERSION.SDK_INT >= 30) {
                    // Some systems only have the list of every app, some
                    // (televisions, Go editions) neither: then the app's page.
                    if (!open(new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:" + getPackageName())))
                            && !open(new Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION))) {
                        openDetails();
                    }
                } else {
                    // Once refused for good the dialog no longer shows: the
                    // app's page in the settings is where to allow it.
                    android.content.SharedPreferences prefs = getPreferences(MODE_PRIVATE);
                    if (prefs.getBoolean("asked", false) && !shouldShowRequestPermissionRationale(Manifest.permission.READ_EXTERNAL_STORAGE)) {
                        openDetails();
                    } else {
                        prefs.edit().putBoolean("asked", true).apply();
                        requestPermissions(new String[] { Manifest.permission.READ_EXTERNAL_STORAGE, Manifest.permission.WRITE_EXTERNAL_STORAGE }, 1);
                    }
                }
            }
        });
    }

    /**
     * The activity is going away (closed, or the system recreates it): the
     * native side neither ends nor starts again in the same process (winit
     * ignores the event, and waiting for it would block here), so the
     * process ends. The recovery copies were written when it left the
     * screen.
     */
    @Override
    protected void onDestroy() {
        System.exit(0);
    }

    /** Starts a settings page; false when the system has none. */
    private boolean open(Intent intent) {
        try {
            startActivity(intent);
            return true;
        } catch (RuntimeException e) {
            return false;
        }
    }

    /** The app's own page in the settings. */
    private void openDetails() {
        open(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + getPackageName())));
    }

    @Override
    protected void onResume() {
        super.onResume();
        hideBars();
        later(300);
    }

    /** Again after a dialog, the keyboard or another app had the focus. */
    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideBars();
            later(300);
        }
    }

    private void later(long ms) {
        View decor = getWindow().getDecorView();
        decor.removeCallbacks(hide);
        decor.postDelayed(hide, ms);
    }

    @SuppressWarnings("deprecation")
    private void keep(WindowInsets insets) {
        int l, t, r, b;
        if (Build.VERSION.SDK_INT >= 30) {
            keyboardHeight = insets.getInsets(WindowInsets.Type.ime()).bottom;
            android.graphics.Insets s = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            l = s.left;
            t = s.top;
            r = s.right;
            b = s.bottom;
        } else {
            l = insets.getSystemWindowInsetLeft();
            t = insets.getSystemWindowInsetTop();
            r = insets.getSystemWindowInsetRight();
            b = insets.getSystemWindowInsetBottom();
            // Below 30 the keyboard is part of the bottom inset.
            keyboardHeight = b > 0 && b > getWindow().getDecorView().getHeight() / 6 ? b : 0;
            if (keyboardHeight > 0) {
                b = 0;
            }
            if (Build.VERSION.SDK_INT >= 28) {
                DisplayCutout c = insets.getDisplayCutout();
                if (c != null) {
                    l = Math.max(l, c.getSafeInsetLeft());
                    t = Math.max(t, c.getSafeInsetTop());
                    r = Math.max(r, c.getSafeInsetRight());
                    b = Math.max(b, c.getSafeInsetBottom());
                }
            }
        }
        insetLeft = l;
        insetTop = t;
        insetRight = r;
        insetBottom = b;
    }

    @SuppressWarnings("deprecation")
    private void hideBars() {
        Window w = getWindow();
        View decor = w.getDecorView();
        // The old flags too: some systems only honour these.
        // Not LAYOUT_STABLE: the insets then count the hidden bars.
        decor.setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_FULLSCREEN
                | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        if (Build.VERSION.SDK_INT >= 30) {
            w.setDecorFitsSystemWindows(false);
            WindowInsetsController bars = decor.getWindowInsetsController();
            if (bars == null) {
                bars = w.getInsetsController();
            }
            if (bars != null) {
                bars.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                bars.hide(WindowInsets.Type.statusBars() | WindowInsets.Type.navigationBars() | WindowInsets.Type.systemBars());
            }
        }
    }
}
