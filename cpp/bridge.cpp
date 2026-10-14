// Ponte C++ verso OpenCascade (OCCT): lettura STEP/IGES con colori e nomi (XDE), assiemi, triangolazione.
// OpenCascade e' distribuito con licenza LGPL 2.1 con l'eccezione OCCT.

#include "bridge.h"

#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRep_Tool.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <IGESCAFControl_Reader.hxx>
#include <IGESCAFControl_Writer.hxx>
#include <Interface_Static.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <STEPCAFControl_Writer.hxx>
#include <STEPControl_StepModelType.hxx>
#include <Standard_Failure.hxx>
#include <Standard_Version.hxx>
#include <TCollection_ExtendedString.hxx>
#include <TDF_Label.hxx>
#include <TDF_LabelSequence.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp_Explorer.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_ColorType.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>

#include <array>
#include <cstdio>
#include <cstring>
#include <map>
#include <string>
#include <vector>

namespace {

void set_err(char* err, uint32_t len, const std::string& m) {
  if (!err || len == 0) return;
  std::snprintf(err, len, "%s", m.c_str());
}

// UTF-16 (nomi di OpenCascade) -> UTF-8
std::string to_utf8(const TCollection_ExtendedString& s) {
  std::string out;
  const Standard_ExtString p = s.ToExtString();
  const int n = s.Length();
  for (int i = 0; i < n; ++i) {
    uint32_t c = p[i];
    if (c >= 0xD800 && c <= 0xDBFF && i + 1 < n) {
      uint32_t d = p[i + 1];
      if (d >= 0xDC00 && d <= 0xDFFF) {
        c = 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00);
        ++i;
      }
    }
    if (c < 0x80) {
      out += static_cast<char>(c);
    } else if (c < 0x800) {
      out += static_cast<char>(0xC0 | (c >> 6));
      out += static_cast<char>(0x80 | (c & 0x3F));
    } else if (c < 0x10000) {
      out += static_cast<char>(0xE0 | (c >> 12));
      out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
      out += static_cast<char>(0x80 | (c & 0x3F));
    } else {
      out += static_cast<char>(0xF0 | (c >> 18));
      out += static_cast<char>(0x80 | ((c >> 12) & 0x3F));
      out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
      out += static_cast<char>(0x80 | (c & 0x3F));
    }
  }
  return out;
}

std::string label_name(const TDF_Label& l) {
  Handle(TDataStd_Name) n;
  if (l.FindAttribute(TDataStd_Name::GetID(), n)) return to_utf8(n->Get());
  return std::string();
}

struct Bucket {
  std::array<float, 4> rgba{{-1.f, -1.f, -1.f, -1.f}};
  std::vector<float> v;
  std::vector<uint32_t> idx;
};

struct Ctx {
  Handle(XCAFDoc_ShapeTool) st;
  Handle(XCAFDoc_ColorTool) ct;
  double lin = 0.1, ang = 0.5;
  rstp_part_cb cb = nullptr;
  void* user = nullptr;
  std::map<std::string, int> seen;  // quante volte e' gia' comparso un nome (istanze dello stesso pezzo)
};

bool color_of(const Ctx& c, const TDF_Label& l, Quantity_Color& out) {
  return c.ct->GetColor(l, XCAFDoc_ColorSurf, out) || c.ct->GetColor(l, XCAFDoc_ColorGen, out);
}

bool color_of(const Ctx& c, const TopoDS_Shape& s, Quantity_Color& out) {
  return c.ct->GetColor(s, XCAFDoc_ColorSurf, out) || c.ct->GetColor(s, XCAFDoc_ColorGen, out);
}

std::array<float, 4> to_rgba(const Quantity_Color& q) {
  double r, g, b;
  q.Values(r, g, b, Quantity_TOC_sRGB);
  return {{static_cast<float>(r), static_cast<float>(g), static_cast<float>(b), 1.f}};
}

void emit(Ctx& c, const TDF_Label& label, const TopLoc_Location& parent, const std::string& name) {
  TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(label);
  if (shape.IsNull()) return;
  BRepMesh_IncrementalMesh mesher(shape, c.lin, Standard_False, c.ang, Standard_True);

  Quantity_Color part_color;
  const bool has_part_color = color_of(c, label, part_color);
  std::map<std::array<int, 4>, Bucket> buckets;

  for (TopExp_Explorer ex(shape, TopAbs_FACE); ex.More(); ex.Next()) {
    const TopoDS_Face face = TopoDS::Face(ex.Current());
    TopLoc_Location face_loc;
    Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, face_loc);
    if (tri.IsNull()) continue;

    std::array<float, 4> rgba{{-1.f, -1.f, -1.f, -1.f}};
    Quantity_Color fc;
    if (color_of(c, face, fc)) rgba = to_rgba(fc);
    else if (has_part_color) rgba = to_rgba(part_color);
    const std::array<int, 4> key{{static_cast<int>(rgba[0] * 255.f + 0.5f), static_cast<int>(rgba[1] * 255.f + 0.5f),
                                 static_cast<int>(rgba[2] * 255.f + 0.5f), static_cast<int>(rgba[3] * 255.f + 0.5f)}};
    Bucket& b = buckets[key];
    b.rgba = rgba;

    const gp_Trsf t = (parent * face_loc).Transformation();
    const bool flip = (face.Orientation() == TopAbs_REVERSED) != t.IsNegative();
    const uint32_t base = static_cast<uint32_t>(b.v.size() / 3);
    for (int i = 1; i <= tri->NbNodes(); ++i) {
      gp_Pnt p = tri->Node(i).Transformed(t);
      b.v.push_back(static_cast<float>(p.X()));
      b.v.push_back(static_cast<float>(p.Y()));
      b.v.push_back(static_cast<float>(p.Z()));
    }
    for (int i = 1; i <= tri->NbTriangles(); ++i) {
      int n1, n2, n3;
      tri->Triangle(i).Get(n1, n2, n3);
      if (flip) std::swap(n2, n3);
      b.idx.push_back(base + static_cast<uint32_t>(n1 - 1));
      b.idx.push_back(base + static_cast<uint32_t>(n2 - 1));
      b.idx.push_back(base + static_cast<uint32_t>(n3 - 1));
    }
  }
  for (auto& kv : buckets) {
    Bucket& b = kv.second;
    if (b.idx.empty()) continue;
    c.cb(c.user, name.c_str(), b.rgba.data(), b.v.size() / 3, b.v.data(), b.idx.size() / 3, b.idx.data());
  }
}

void visit(Ctx& c, const TDF_Label& label, const TopLoc_Location& parent, const std::string& path, int depth) {
  if (depth > 64) return;  // un assieme che contiene se' stesso
  if (XCAFDoc_ShapeTool::IsReference(label)) {
    TDF_Label ref;
    if (!XCAFDoc_ShapeTool::GetReferredShape(label, ref)) return;
    visit(c, ref, parent * XCAFDoc_ShapeTool::GetLocation(label), path, depth + 1);
    return;
  }
  std::string n = label_name(label);
  if (n.empty()) n = "Parte";
  const std::string full = path.empty() ? n : path + "/" + n;
  if (XCAFDoc_ShapeTool::IsAssembly(label)) {
    TDF_LabelSequence comps;
    XCAFDoc_ShapeTool::GetComponents(label, comps);
    for (Standard_Integer i = 1; i <= comps.Length(); ++i) visit(c, comps.Value(i), parent, full, depth + 1);
  } else if (XCAFDoc_ShapeTool::IsSimpleShape(label)) {
    const int k = ++c.seen[full];
    emit(c, label, parent, k == 1 ? full : full + " (" + std::to_string(k) + ")");
  }
}

Handle(TDocStd_Document) new_doc() {
  Handle(XCAFApp_Application) app = XCAFApp_Application::GetApplication();
  Handle(TDocStd_Document) doc;
  app->NewDocument(TCollection_ExtendedString("MDTV-XCAF"), doc);
  return doc;
}

}  // namespace

extern "C" {

const char* rstp_occt_version(void) { return OCC_VERSION_STRING_EXT; }

int rstp_load(const char* path, int format, double lin, double ang, rstp_part_cb cb, void* user, char* err,
              uint32_t err_len) {
  try {
    Interface_Static::SetCVal("xstep.cascade.unit", "MM");
    Handle(TDocStd_Document) doc = new_doc();
    if (format == 1) {
      IGESCAFControl_Reader r;
      r.SetColorMode(Standard_True);
      r.SetNameMode(Standard_True);
      if (r.ReadFile(path) != IFSelect_RetDone) {
        set_err(err, err_len, "il file IGES non si apre o non e' valido");
        return 2;
      }
      if (!r.Transfer(doc)) {
        set_err(err, err_len, "il file IGES non contiene forme che si possano convertire");
        return 3;
      }
    } else {
      STEPCAFControl_Reader r;
      r.SetColorMode(Standard_True);
      r.SetNameMode(Standard_True);
      r.SetLayerMode(Standard_False);
      if (r.ReadFile(path) != IFSelect_RetDone) {
        set_err(err, err_len, "il file STEP non si apre o non e' valido");
        return 2;
      }
      if (!r.Transfer(doc)) {
        set_err(err, err_len, "il file STEP non contiene forme che si possano convertire");
        return 3;
      }
    }
    Ctx c;
    c.st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    c.ct = XCAFDoc_DocumentTool::ColorTool(doc->Main());
    c.lin = lin;
    c.ang = ang;
    c.cb = cb;
    c.user = user;
    TDF_LabelSequence roots;
    c.st->GetFreeShapes(roots);
    if (roots.Length() == 0) {
      set_err(err, err_len, "il file non contiene forme");
      return 4;
    }
    for (Standard_Integer i = 1; i <= roots.Length(); ++i) visit(c, roots.Value(i), TopLoc_Location(), std::string(), 0);
    return 0;
  } catch (const Standard_Failure& e) {
    set_err(err, err_len, std::string("OpenCascade: ") + (e.GetMessageString() ? e.GetMessageString() : e.DynamicType()->Name()));
    return 10;
  } catch (const std::exception& e) {
    set_err(err, err_len, std::string("errore: ") + e.what());
    return 11;
  } catch (...) {
    set_err(err, err_len, "errore sconosciuto in OpenCascade");
    return 12;
  }
}

int rstp_write_test_box(const char* path, int format, char* err, uint32_t err_len) {
  try {
    Handle(TDocStd_Document) doc = new_doc();
    Handle(XCAFDoc_ShapeTool) st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
    Handle(XCAFDoc_ColorTool) ct = XCAFDoc_DocumentTool::ColorTool(doc->Main());

    TDF_Label red = st->AddShape(BRepPrimAPI_MakeBox(20.0, 10.0, 5.0).Shape(), Standard_False);
    TDF_Label blue = st->AddShape(BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape(), Standard_False);
    TDataStd_Name::Set(red, TCollection_ExtendedString("CuboRosso"));
    TDataStd_Name::Set(blue, TCollection_ExtendedString("CuboBlu"));
    ct->SetColor(red, Quantity_Color(1.0, 0.0, 0.0, Quantity_TOC_sRGB), XCAFDoc_ColorSurf);
    ct->SetColor(blue, Quantity_Color(0.0, 0.0, 1.0, Quantity_TOC_sRGB), XCAFDoc_ColorSurf);

    TDF_Label assembly = st->NewShape();
    TDataStd_Name::Set(assembly, TCollection_ExtendedString("Assieme"));
    gp_Trsf move;
    move.SetTranslation(gp_Vec(30.0, 0.0, 0.0));
    st->AddComponent(assembly, red, TopLoc_Location());
    st->AddComponent(assembly, blue, TopLoc_Location(move));
    st->UpdateAssemblies();

    if (format == 1) {
      IGESCAFControl_Writer w;
      w.SetColorMode(Standard_True);
      w.SetNameMode(Standard_True);
      if (!w.Transfer(doc)) {
        set_err(err, err_len, "scrittura IGES di prova non riuscita");
        return 2;
      }
      if (!w.Write(path)) {
        set_err(err, err_len, "impossibile scrivere il file IGES di prova");
        return 3;
      }
    } else {
      STEPCAFControl_Writer w;
      w.SetColorMode(Standard_True);
      w.SetNameMode(Standard_True);
      if (!w.Transfer(doc, STEPControl_AsIs)) {
        set_err(err, err_len, "scrittura STEP di prova non riuscita");
        return 2;
      }
      if (w.Write(path) != IFSelect_RetDone) {
        set_err(err, err_len, "impossibile scrivere il file STEP di prova");
        return 3;
      }
    }
    return 0;
  } catch (const Standard_Failure& e) {
    set_err(err, err_len, std::string("OpenCascade: ") + (e.GetMessageString() ? e.GetMessageString() : e.DynamicType()->Name()));
    return 10;
  } catch (...) {
    set_err(err, err_len, "errore sconosciuto in OpenCascade");
    return 12;
  }
}

}  // extern "C"
