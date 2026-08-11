import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.ColorSpace;
import java.io.FileOutputStream;

/** Generates the synthetic Android encoder fixtures documented in this directory. */
public final class GenerateFixtures {
  private static void write(Bitmap bitmap, Bitmap.CompressFormat format, String stem)
      throws Exception {
    String extension = format == Bitmap.CompressFormat.PNG ? "png" : "jpg";
    try (FileOutputStream output =
        new FileOutputStream("/data/local/tmp/" + stem + "." + extension)) {
      if (!bitmap.compress(format, 100, output)) {
        throw new IllegalStateException("Bitmap.compress failed for " + stem);
      }
    }
  }

  public static void main(String[] args) throws Exception {
    Bitmap srgb = Bitmap.createBitmap(3, 2, Bitmap.Config.ARGB_8888);
    srgb.setPixels(
        new int[] {
          Color.argb(255, 255, 0, 0),
          Color.argb(255, 0, 255, 0),
          Color.argb(255, 0, 0, 255),
          Color.argb(128, 255, 255, 0),
          Color.argb(64, 0, 255, 255),
          Color.argb(0, 255, 0, 255),
        },
        0,
        3,
        0,
        0,
        3,
        2);
    write(srgb, Bitmap.CompressFormat.PNG, "bitmap-srgb");
    write(srgb, Bitmap.CompressFormat.JPEG, "bitmap-srgb");

    Bitmap displayP3 =
        Bitmap.createBitmap(
            3,
            2,
            Bitmap.Config.RGBA_F16,
            true,
            ColorSpace.get(ColorSpace.Named.DISPLAY_P3));
    displayP3.eraseColor(
        Color.pack(
            1.0f,
            0.0f,
            0.0f,
            1.0f,
            ColorSpace.get(ColorSpace.Named.DISPLAY_P3)));
    write(displayP3, Bitmap.CompressFormat.PNG, "bitmap-display-p3");
    write(displayP3, Bitmap.CompressFormat.JPEG, "bitmap-display-p3");
  }

  private GenerateFixtures() {}
}
