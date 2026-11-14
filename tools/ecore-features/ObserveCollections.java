// Handwritten independent EMF observation host. Never a production dependency.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.ecore.*;

public class ObserveCollections {
  static Map<String,Object> record(Object... pairs) {
    Map<String,Object> result=new LinkedHashMap<>();
    for(int n=0;n<pairs.length;n+=2) result.put((String)pairs[n],pairs[n+1]);
    return result;
  }
  static String json(Object value) {
    if(value==null) return "null";
    if(value instanceof Map<?,?> map) return "{"+String.join(",",map.entrySet().stream().map(e->json(e.getKey())+":"+json(e.getValue())).toList())+"}";
    if(value instanceof Collection<?> list) return "["+String.join(",",list.stream().map(ObserveCollections::json).toList())+"]";
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
    for(int mask=0;mask<16;mask++) {
      EcoreFactory factory=EcoreFactory.eINSTANCE;
      EPackage pack=factory.createEPackage();pack.setName("lists");pack.setNsPrefix("lists");pack.setNsURI("urn:intersect:e09:list:"+mask);
      EClass type=factory.createEClass();type.setName("Item");pack.getEClassifiers().add(type);
      EAttribute feature=factory.createEAttribute();feature.setName("value");feature.setEType(EcorePackage.Literals.ESTRING);feature.setUpperBound(-1);
      feature.setChangeable((mask&1)!=0);feature.setUnsettable((mask&2)!=0);feature.setUnique((mask&4)!=0);feature.setOrdered((mask&8)!=0);
      type.getEStructuralFeatures().add(feature);EObject object=pack.getEFactoryInstance().create(type);
      List<Object> values=(List<Object>)object.eGet(feature);List<Object> steps=new ArrayList<>();
      step(steps,"fresh",object,feature,()->{});
      step(steps,"remove-missing-fresh",object,feature,()->values.remove("missing"));
      step(steps,"clear-fresh",object,feature,()->values.clear());
      step(steps,"add-a",object,feature,()->values.add("a"));
      step(steps,"add-a-again",object,feature,()->values.add("a"));
      step(steps,"add-b",object,feature,()->values.add("b"));
      step(steps,"remove-a",object,feature,()->values.remove("a"));
      step(steps,"remove-a-again",object,feature,()->values.remove("a"));
      step(steps,"remove-b",object,feature,()->values.remove("b"));
      step(steps,"clear-empty",object,feature,()->values.clear());
      step(steps,"reset",object,feature,()->object.eUnset(feature));
      cases.add(record("mask",mask,"changeable",feature.isChangeable(),"unsettable",feature.isUnsettable(),"unique",feature.isUnique(),"ordered",feature.isOrdered(),"steps",steps));
    }
    Files.writeString(Path.of(args[0]),json(record("scope","Dynamic EString collection mutation and state; no generated getters or persistence", "cases",cases))+"\n");
  }
}
