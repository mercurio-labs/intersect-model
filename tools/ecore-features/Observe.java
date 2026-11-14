// Handwritten independent EMF observation host. Never a production dependency.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.ecore.*;

public class Observe {
  static Map<String,Object> record(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int n=0;n<pairs.length;n+=2) result.put((String)pairs[n],pairs[n+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(Observe::json).toList())+"]";
    if(value instanceof Boolean || value instanceof Number) return value.toString();
    return "\""+value.toString().replace("\\","\\\\").replace("\"","\\\"").replace("\n","\\n").replace("\r","\\r").replace("\t","\\t")+"\"";
  }
  static void step(List<Object> steps,String name,EObject object,EStructuralFeature feature,Runnable action) {
    Map<String,Object> row=record("operation",name);
    try { action.run(); row.put("accepted",true); }
    catch(RuntimeException error) { row.put("accepted",false);row.put("error",error.getClass().getName()); }
    try {
      Object value=object.eGet(feature);
      row.put("value",value instanceof Collection<?> values ? new ArrayList<>(values) : value);
    } catch(RuntimeException error) { row.put("read_error",error.getClass().getName()); }
    try { row.put("is_set",object.eIsSet(feature)); }
    catch(RuntimeException error) { row.put("state_error",error.getClass().getName()); }
    steps.add(row);
  }
  @SuppressWarnings("unchecked")
  public static void main(String[] args) throws Exception {
    List<Object> cases=new ArrayList<>();
    for(int mask=0;mask<32;mask++) for(boolean many:List.of(false,true)) {
      EcoreFactory factory=EcoreFactory.eINSTANCE;
      EPackage pack=factory.createEPackage();pack.setName("flags");pack.setNsPrefix("flags");pack.setNsURI("urn:intersect:e09:"+mask+":"+many);
      EClass type=factory.createEClass();type.setName("Item");pack.getEClassifiers().add(type);
      EAttribute feature=factory.createEAttribute();feature.setName("value");feature.setEType(EcorePackage.Literals.ESTRING);
      feature.setUpperBound(many ? -1 : 1);
      feature.setChangeable((mask&1)!=0);feature.setDerived((mask&2)!=0);feature.setVolatile((mask&4)!=0);
      feature.setTransient((mask&8)!=0);feature.setUnsettable((mask&16)!=0);
      if(!many) feature.setDefaultValueLiteral("seed");
      type.getEStructuralFeatures().add(feature);
      EObject object=pack.getEFactoryInstance().create(type);
      List<Object> steps=new ArrayList<>();
      step(steps,"fresh",object,feature,()->{});
      step(steps,"explicit-default",object,feature,()->object.eSet(feature,many ? List.of() : "seed"));
      step(steps,"unset-default",object,feature,()->object.eUnset(feature));
      step(steps,"explicit-value",object,feature,()->object.eSet(feature,many ? List.of("custom") : "custom"));
      if(many) {
        step(steps,"clear-list",object,feature,()->((List<Object>)object.eGet(feature)).clear());
        step(steps,"add-list",object,feature,()->((List<Object>)object.eGet(feature)).add("next"));
        step(steps,"remove-last",object,feature,()->((List<Object>)object.eGet(feature)).remove("next"));
      } else {
        step(steps,"explicit-null",object,feature,()->object.eSet(feature,null));
      }
      step(steps,"unset",object,feature,()->object.eUnset(feature));
      step(steps,"unset-again",object,feature,()->object.eUnset(feature));
      cases.add(record("mask",mask,"many",many,"changeable",feature.isChangeable(),"derived",feature.isDerived(),"volatile",feature.isVolatile(),"transient",feature.isTransient(),"unsettable",feature.isUnsettable(),"steps",steps));
    }
    Files.writeString(Path.of(args[0]),json(record("scope","Dynamic EString feature flags and assignment-state transitions only; no generated getters or persistence", "cases",cases))+"\n");
  }
}
