package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ImageViewerGeometryTest {
 @Test fun fittedImageCannotPanAway(){assertEquals(ImageTransform(),constrainImage(ImageTransform(0.1f,99f,-99f),400f,600f,800f,400f))}
 @Test fun zoomBoundsRespectLetterboxing(){assertEquals(ImageTransform(5f,800f,200f),constrainImage(ImageTransform(20f,9999f,9999f),400f,600f,800f,400f))}
 @Test fun pinchKeepsFocalPointAndPansInsideBounds(){assertEquals(ImageTransform(2f,-100f,0f),imageGesture(ImageTransform(),2f,0f,0f,300f,300f,400f,600f,400f,600f))}
 @Test fun resetRemovesPanAndInvalidGeometry(){assertEquals(ImageTransform(),constrainImage(ImageTransform(1f,50f,60f),400f,600f,400f,600f));assertEquals(ImageTransform(),constrainImage(ImageTransform(3f),0f,0f,800f,800f))}
 @Test fun thumbnailsDecodeLessThanPreview(){assertEquals(4,bitmapSample(2400,1600,720));assertEquals(2,bitmapSample(2400,1600,1600));assertEquals(1,bitmapSample(400,600,720))}
}
