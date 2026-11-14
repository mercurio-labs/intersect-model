// Independent optional EMF oracle. Never used in native/browser production builds.
import java.nio.file.*;
import java.util.*;
import org.eclipse.emf.common.util.URI;
import org.eclipse.emf.ecore.*;
import org.eclipse.emf.ecore.resource.*;
import org.eclipse.emf.ecore.resource.impl.ResourceSetImpl;
import org.eclipse.emf.ecore.util.EcoreUtil;
import org.eclipse.emf.ecore.xmi.impl.*;

class Verify {
  public static void main(String[] args) throws Exception {
    Path root = Path.of(args[0]).toAbsolutePath();
    Path input = Path.of(args[1]).toAbsolutePath();
    Path output = Path.of(args[2]).toAbsolutePath();
    Files.createDirectories(output);
    ResourceSet set = new ResourceSetImpl();
    set.getResourceFactoryRegistry().getExtensionToFactoryMap().put("ecore", new EcoreResourceFactoryImpl());
    set.getResourceFactoryRegistry().getExtensionToFactoryMap().put("xmi", new XMIResourceFactoryImpl());
    Resource definition = set.getResource(URI.createFileURI(root.resolve("fixtures/library/library.ecore").toString()), true);
    EPackage schema = (EPackage)definition.getContents().get(0);
    set.getPackageRegistry().put(schema.getNsURI(), schema);
    List<Resource> models = new ArrayList<>();
    try (var paths = Files.list(input)) {
      for (Path path : paths.filter(p -> p.toString().endsWith(".xmi")).sorted().toList()) {
        models.add(set.getResource(URI.createFileURI(path.toString()), true));
      }
    }
    EcoreUtil.resolveAll(set);
    int count = 0; int loans = 0;
    for (Resource resource : models) {
      if (!resource.getErrors().isEmpty()) throw new IllegalStateException(resource.getErrors().toString());
      for (var it = resource.getAllContents(); it.hasNext();) {
        EObject object = it.next(); count++;
        for (EReference reference : object.eClass().getEAllReferences()) {
          Object value = object.eGet(reference);
          List<?> targets = reference.isMany() ? (List<?>)value : value == null ? List.of() : List.of(value);
          for (Object target : targets) if (((EObject)target).eIsProxy()) throw new IllegalStateException("Unresolved " + reference.getName() + ": " + ((InternalEObject)target).eProxyURI() + "; first resource e1=" + models.get(0).getEObject("e1"));
        }
        if (object.eClass().getName().equals("Loan")) {
          loans++;
          EObject copy = (EObject)object.eGet(object.eClass().getEStructuralFeature("copy"));
          if (!"BookCopy".equals(copy.eClass().getName())) throw new IllegalStateException("Wrong loan copy");
        }
      }
    }
    if (count != 6 || loans != 1) throw new IllegalStateException("Unexpected model counts: " + count + "/" + loans);
    // Relocate all resource URIs together before saving cross-resource hrefs.
    for (Resource resource : models) resource.setURI(URI.createFileURI(output.resolve(resource.getURI().lastSegment()).toString()));
    for (Resource resource : models) {
      // Emit plain EMF interchange to exercise import without Intersect metadata.
      ((org.eclipse.emf.ecore.xmi.XMLResource)resource).getEObjectToExtensionMap().clear();
      resource.save(Map.of());
    }
    System.out.println("EMF loaded and resolved 6 elements across " + models.size() + " resources; saved for native reimport.");
  }
}
