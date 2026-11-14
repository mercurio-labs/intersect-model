// Independent validity and duplicate-opposite observations; verification only.
// Verification only; never linked into normal builds or production.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.common.util.Diagnostic;
import org.eclipse.emf.ecore.*;
import org.eclipse.emf.ecore.util.Diagnostician;
public class ObserveNonuniqueOpposites {
  static Map<String,Object> row(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int i=0;i<pairs.length;i+=2) result.put((String)pairs[i],pairs[i+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveNonuniqueOpposites::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"")+"\"";
  }

  static Object value(Object v,Map<EObject,String> ids) {
    if(v instanceof EObject o) return ids.get(o);
    if(v instanceof Collection<?> list) return list.stream().map(x->value(x,ids)).toList();
    return v;
  }
  static Object state(Map<String,EObject> objects,Map<EObject,String> ids,EReference out,EReference back) {
    Map<String,Object> result=new LinkedHashMap<>();
    objects.forEach((name,o)->{ var feature=name.startsWith("a")?out:back;
      result.put(name,row("value",value(o.eGet(feature),ids),"is_set",o.eIsSet(feature))); });
    return result;
  }
  @SuppressWarnings("unchecked")
  static void add(EObject o,EReference f,EObject target) {
    if(f.isMany()) ((List<EObject>)o.eGet(f)).add(target); else o.eSet(f,target);
  }
  @SuppressWarnings("unchecked")
  static void remove(EObject o,EReference f,EObject target) {
    if(f.isMany()) ((List<EObject>)o.eGet(f)).remove(target); else o.eUnset(f);
  }
  static Object diagnostics(Diagnostic d) {
    return row("severity",d.getSeverity(),"source",d.getSource(),"code",d.getCode(),"message",d.getChildren().isEmpty()?d.getMessage():null,"children",d.getChildren().stream().map(ObserveNonuniqueOpposites::diagnostics).toList());
  }
  public static void main(String[] args) throws Exception {
    List<Object> cases=new ArrayList<>();
    for(boolean om:List.of(false,true)) for(boolean bm:List.of(false,true))
    for(boolean ou:List.of(false,true)) for(boolean bu:List.of(false,true))
    for(boolean containment:List.of(false,true)) for(boolean unsettable:List.of(false,true)) {
      var f=EcoreFactory.eINSTANCE;
      var p=f.createEPackage();p.setName("duplicates");p.setNsPrefix("d");p.setNsURI("urn:duplicates");
      var a=f.createEClass();a.setName("A");p.getEClassifiers().add(a);
      var b=f.createEClass();b.setName("B");p.getEClassifiers().add(b);
      var out=f.createEReference();out.setName("out");out.setEType(b);out.setUpperBound(om?-1:1);out.setUnique(ou);out.setContainment(containment);out.setUnsettable(unsettable);a.getEStructuralFeatures().add(out);
      var back=f.createEReference();back.setName("back");back.setEType(a);back.setUpperBound(bm?-1:1);back.setUnique(bu);back.setUnsettable(unsettable);b.getEStructuralFeatures().add(back);
      out.setEOpposite(back);back.setEOpposite(out);
      var diagnostic=Diagnostician.INSTANCE.validate(p);
      boolean valid=diagnostic.getSeverity()<Diagnostic.ERROR;
      var entry=row("out_many",om,"back_many",bm,"out_unique",ou,"back_unique",bu,"containment",containment,"unsettable",unsettable,"valid",valid,"diagnostic",diagnostics(diagnostic));
      List<Object> steps=new ArrayList<>();entry.put("steps",steps);cases.add(entry);
      if(!valid) continue;
      Map<String,EObject> objects=new LinkedHashMap<>();Map<EObject,String> ids=new IdentityHashMap<>();
      for(String id:List.of("a1","a2","b1","b2")) {var o=p.getEFactoryInstance().create(id.startsWith("a")?a:b);objects.put(id,o);ids.put(o,id);}
      var a1=objects.get("a1");var a2=objects.get("a2");var b1=objects.get("b1");
      steps.add(row("operation","fresh","objects",state(objects,ids,out,back)));
      add(a1,out,b1);steps.add(row("operation","add-first","objects",state(objects,ids,out,back)));
      add(a1,out,b1);steps.add(row("operation","add-duplicate","objects",state(objects,ids,out,back)));
      add(b1,back,a2);steps.add(row("operation","add-reverse","objects",state(objects,ids,out,back)));
      remove(a1,out,b1);steps.add(row("operation","remove-one","objects",state(objects,ids,out,back)));
      b1.eUnset(back);steps.add(row("operation","unset-reverse","objects",state(objects,ids,out,back)));
    }
    Files.writeString(Path.of(args[0]),json(row("schema_version",1,"cases",cases))+"\n");
  }
}
