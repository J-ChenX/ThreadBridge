package dev.threadbridge.qa;

import android.os.SystemClock;
import android.view.InputDevice;
import android.view.InputEvent;
import android.view.MotionEvent;
import java.lang.reflect.Method;

/** Shell-only test driver: send a real two-pointer pinch without adb process delays. */
public final class ImageGestures {
 private static Object manager;
 private static Method inject;
 private static long down;
 private static void event(int action,float[] xs,float[] ys) throws Exception {
  MotionEvent.PointerProperties[] props=new MotionEvent.PointerProperties[xs.length];
  MotionEvent.PointerCoords[] coords=new MotionEvent.PointerCoords[xs.length];
  for(int i=0;i<xs.length;i++){
   props[i]=new MotionEvent.PointerProperties();props[i].id=i;props[i].toolType=MotionEvent.TOOL_TYPE_FINGER;
   coords[i]=new MotionEvent.PointerCoords();coords[i].x=xs[i];coords[i].y=ys[i];coords[i].pressure=1;coords[i].size=1;
  }
  MotionEvent e=MotionEvent.obtain(down,SystemClock.uptimeMillis(),action,xs.length,props,coords,0,0,1,1,0,0,InputDevice.SOURCE_TOUCHSCREEN,0);
  if(!Boolean.TRUE.equals(inject.invoke(manager,e,2)))throw new AssertionError("Input injection failed");
  e.recycle();
 }
 public static void main(String[] args) throws Exception {
  Class<?> type=Class.forName("android.hardware.input.InputManagerGlobal");manager=type.getMethod("getInstance").invoke(null);inject=type.getMethod("injectInputEvent",InputEvent.class,int.class);
  float x=Float.parseFloat(args[1]),y=Float.parseFloat(args[2]);down=SystemClock.uptimeMillis();
  if(args[0].equals("double")){
   for(int i=0;i<2;i++){down=SystemClock.uptimeMillis();event(0,new float[]{x},new float[]{y});Thread.sleep(40);event(1,new float[]{x},new float[]{y});Thread.sleep(80);}
  }else if(args[0].equals("pinch")){
   event(0,new float[]{x-100},new float[]{y});event(5|(1<<8),new float[]{x-100,x+100},new float[]{y,y});
   for(int i=1;i<=12;i++){Thread.sleep(20);float d=100+i*10;event(2,new float[]{x-d,x+d},new float[]{y,y});}
   event(6|(1<<8),new float[]{x-220,x+220},new float[]{y,y});event(1,new float[]{x-220},new float[]{y});
  }else throw new AssertionError("Unknown gesture");
 }
}
