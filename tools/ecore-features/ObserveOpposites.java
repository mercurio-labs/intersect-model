// Independent dynamic EMF inverse-reference observations; verification only.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.ecore.*;
public class ObserveOpposites {
  static Map<String,Object> record(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int n=0;n<pairs.length;n+=2) result.put((String)pairs[n],pairs[n+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveOpposites::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"").replace("\n","\\n").replace("\r","\\r").replace("\t","\\t")+"\"";
  }

  static Object ids(Object value, IdentityHashMap<EObject,String> ids) {
    if(value instanceof EObject object) return ids.get(object);
    if(value instanceof Collection<?> values) return values.stream().map(v->ids(v,ids)).toList();
    return value;
  }
  static void step(List<Object> steps,String name,Map<String,EObject> objects,
      IdentityHashMap<EObject,String> ids,Runnable action) {
    Map<String,Object> row=record("operation",name);
    try {action.run();row.put("accepted",true);}
    catch(RuntimeException error) {row.put("accepted",false);row.put("error",error.getClass().getName());}
    Map<String,Object> states=new LinkedHashMap<>();
    for(var entry:objects.entrySet()) {
      EObject object=entry.getValue();EStructuralFeature field=object.eClass().getEStructuralFeatures().get(0);
      states.put(entry.getKey(),record("value",ids(object.eGet(field),ids),"is_set",object.eIsSet(field),"container",ids(object.eContainer(),ids)));
    }
    row.put("objects",states);steps.add(row);
  }
  static void assign(EObject owner,EReference feature,EObject value) {
    owner.eSet(feature,feature.isMany()?(value==null?List.of():List.of(value)):value);
  }
  public static void main(String[] args) throws Exception {
    List<Object> cases=new ArrayList<>();
    for(boolean containment:List.of(false,true)) for(boolean outMany:List.of(false,true)) for(boolean backMany:List.of(false,true)) {
      if(containment && backMany) continue;
      for(int flags=0;flags<4;flags++) {
        EcoreFactory f=EcoreFactory.eINSTANCE;
        EPackage pack=f.createEPackage();pack.setName("inverse");pack.setNsURI("urn:inverse");pack.setNsPrefix("inverse");
        EClass a=f.createEClass();a.setName("A");EClass b=f.createEClass();b.setName("B");pack.getEClassifiers().addAll(List.of(a,b));
        EReference out=f.createEReference();out.setName("out");out.setEType(b);out.setUpperBound(outMany?-1:1);out.setContainment(containment);out.setUnsettable((flags&1)!=0);a.getEStructuralFeatures().add(out);
        EReference back=f.createEReference();back.setName("back");back.setEType(a);back.setUpperBound(backMany?-1:1);back.setUnsettable((flags&2)!=0);b.getEStructuralFeatures().add(back);
        out.setEOpposite(back);back.setEOpposite(out);
        Map<String,EObject> objects=new LinkedHashMap<>();IdentityHashMap<EObject,String> ids=new IdentityHashMap<>();
        for(String name:List.of("a1","a2","b1","b2")) {EObject object=pack.getEFactoryInstance().create(name.startsWith("a")?a:b);objects.put(name,object);ids.put(object,name);}
        EObject a1=objects.get("a1"),a2=objects.get("a2"),b1=objects.get("b1"),b2=objects.get("b2");
        List<Object> steps=new ArrayList<>();
        step(steps,"fresh",objects,ids,()->{});
        step(steps,"explicit-empty",objects,ids,()->assign(a1,out,null));
        step(steps,"bind-first",objects,ids,()->assign(a1,out,b1));
        step(steps,"bind-again",objects,ids,()->assign(a1,out,b1));
        step(steps,"replace-target",objects,ids,()->assign(a1,out,b2));
        step(steps,"unset-forward",objects,ids,()->a1.eUnset(out));
        step(steps,"bind-reverse",objects,ids,()->assign(b1,back,a1));
        step(steps,"unset-reverse",objects,ids,()->b1.eUnset(back));
        step(steps,"bind-forward",objects,ids,()->assign(a1,out,b1));
        step(steps,"move-owner",objects,ids,()->assign(a2,out,b1));
        step(steps,"empty-forward",objects,ids,()->assign(a2,out,null));
        step(steps,"unset-forward-again",objects,ids,()->a2.eUnset(out));
        cases.add(record("containment",containment,"out_many",outMany,"back_many",backMany,"out_unsettable",out.isUnsettable(),"back_unsettable",back.isUnsettable(),"steps",steps));
      }
    }
    Files.writeString(Path.of(args[0]),json(record("schema_version",1,"cases",cases))+"\n");
  }
}
