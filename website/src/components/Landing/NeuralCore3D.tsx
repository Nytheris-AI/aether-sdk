import {useEffect, useRef, useState, type ReactNode} from 'react';
import * as THREE from 'three';
import {OrbitControls} from 'three/examples/jsm/controls/OrbitControls.js';
import {RoomEnvironment} from 'three/examples/jsm/environments/RoomEnvironment.js';
import {GLTFLoader} from 'three/examples/jsm/loaders/GLTFLoader.js';
import {GLB_MODEL_URL} from '../../data/landing';

function buildCircleLine(
  radius: number,
  segments: number,
  color: number,
  opacity: number,
): THREE.Line {
  const pts: THREE.Vector3[] = [];
  for (let i = 0; i <= segments; i++) {
    const theta = (i / segments) * Math.PI * 2;
    pts.push(new THREE.Vector3(Math.cos(theta) * radius, 0, Math.sin(theta) * radius));
  }
  const geo = new THREE.BufferGeometry().setFromPoints(pts);
  const mat = new THREE.LineBasicMaterial({color, transparent: true, opacity});
  return new THREE.Line(geo, mat);
}

export function NeuralCore3D(): ReactNode {
  const mountRef = useRef<HTMLDivElement>(null);
  const [wireframeOnly, setWireframeOnly] = useState(false);
  const [statusText, setStatusText] = useState('DRAG TO ORBIT 3D CORE');

  const materialsRef = useRef<THREE.MeshStandardMaterial[]>([]);
  const edgeLinesRef = useRef<THREE.LineSegments[]>([]);
  const renderOnceRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    for (const mat of materialsRef.current) {
      mat.wireframe = wireframeOnly;
      mat.color.setHex(wireframeOnly ? 0x2248ff : 0xf7f7f4);
      const hasEmissiveMap = Boolean(mat.userData.hasEmissiveMap);
      mat.emissiveIntensity = wireframeOnly ? 0.35 : hasEmissiveMap ? 2.1 : 0.03;
    }
    for (const line of edgeLinesRef.current) {
      line.visible = !wireframeOnly;
    }
    renderOnceRef.current?.();
  }, [wireframeOnly]);

  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;

    let disposed = false;
    const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const scene = new THREE.Scene();

    const camera = new THREE.PerspectiveCamera(38, 1, 0.1, 100);
    camera.position.set(0, 0.15, 4.75);

    let renderer: THREE.WebGLRenderer;
    try {
      renderer = new THREE.WebGLRenderer({
        antialias: true,
        alpha: true,
        powerPreference: 'high-performance',
      });
    } catch {
      setStatusText('3D PREVIEW UNAVAILABLE');
      return;
    }

    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.12;
    mount.appendChild(renderer.domElement);

    const pmrem = new THREE.PMREMGenerator(renderer);
    const roomEnv = new RoomEnvironment();
    const envTarget = pmrem.fromScene(roomEnv, 0.04);
    scene.environment = envTarget.texture;

    scene.add(new THREE.AmbientLight(0xf7f7f4, 0.9));

    const keyLight = new THREE.DirectionalLight(0xffffff, 1.9);
    keyLight.position.set(3.5, 4.5, 4.0);
    scene.add(keyLight);

    const cobaltRimLight = new THREE.DirectionalLight(0x2248ff, 2.4);
    cobaltRimLight.position.set(-4.0, 1.8, -2.5);
    scene.add(cobaltRimLight);

    const bottomFill = new THREE.DirectionalLight(0x2248ff, 0.65);
    bottomFill.position.set(0, -3.5, 2.0);
    scene.add(bottomFill);

    const controls = new OrbitControls(camera, renderer.domElement);
    controls.enableDamping = true;
    controls.dampingFactor = 0.06;
    controls.enableZoom = false;
    controls.enablePan = false;
    controls.rotateSpeed = 0.75;
    controls.minPolarAngle = Math.PI * 0.24;
    controls.maxPolarAngle = Math.PI * 0.76;

    const rootGroup = new THREE.Group();
    scene.add(rootGroup);

    const modelGroup = new THREE.Group();
    rootGroup.add(modelGroup);

    const orbitGroup = new THREE.Group();
    rootGroup.add(orbitGroup);

    const outerRing = buildCircleLine(1.78, 96, 0x111315, 0.22);
    outerRing.rotation.x = Math.PI * 0.42;
    outerRing.rotation.z = Math.PI * 0.12;
    orbitGroup.add(outerRing);

    const cobaltRing = buildCircleLine(1.52, 96, 0x2248ff, 0.48);
    cobaltRing.rotation.x = -Math.PI * 0.34;
    cobaltRing.rotation.y = Math.PI * 0.18;
    orbitGroup.add(cobaltRing);

    const satGeo = new THREE.BoxGeometry(0.075, 0.075, 0.075);
    const satCobaltMat = new THREE.MeshBasicMaterial({color: 0x2248ff});
    const satInkMat = new THREE.MeshBasicMaterial({color: 0x111315});

    for (let i = 0; i < 4; i++) {
      const angle = (i / 4) * Math.PI * 2;
      const sat = new THREE.Mesh(satGeo, i % 2 === 0 ? satCobaltMat : satInkMat);
      sat.position.set(Math.cos(angle) * 1.52, 0, Math.sin(angle) * 1.52);
      cobaltRing.add(sat);
    }

    const fallbackCoreGroup = new THREE.Group();
    const icoGeo = new THREE.IcosahedronGeometry(0.95, 1);
    const icoMat = new THREE.MeshStandardMaterial({
      color: 0xf7f7f4,
      roughness: 0.32,
      metalness: 0.12,
      flatShading: true,
    });
    const icoMesh = new THREE.Mesh(icoGeo, icoMat);
    const icoEdgesMat = new THREE.LineBasicMaterial({
      color: 0x2248ff,
      transparent: true,
      opacity: 0.65,
    });
    const icoEdges = new THREE.LineSegments(new THREE.EdgesGeometry(icoGeo), icoEdgesMat);
    fallbackCoreGroup.add(icoMesh, icoEdges);
    modelGroup.add(fallbackCoreGroup);

    materialsRef.current = [icoMat];
    edgeLinesRef.current = [icoEdges];

    renderOnceRef.current = () => {
      if (!disposed) {
        renderer.render(scene, camera);
      }
    };

    const loader = new GLTFLoader();
    setStatusText('LOADING 3D GLB MESH...');

    loader.load(
      GLB_MODEL_URL,
      (gltf) => {
        if (disposed) return;
        const loadedRoot = gltf.scene;
        const box = new THREE.Box3().setFromObject(loadedRoot);
        const size = box.getSize(new THREE.Vector3());
        const center = box.getCenter(new THREE.Vector3());
        const maxDim = Math.max(size.x, size.y, size.z) || 1;
        const targetScale = 2.15 / maxDim;

        loadedRoot.position.sub(center);
        const wrapper = new THREE.Group();
        wrapper.scale.setScalar(targetScale);
        wrapper.add(loadedRoot);

        const nextMats: THREE.MeshStandardMaterial[] = [];
        const nextEdges: THREE.LineSegments[] = [];

        loadedRoot.traverse((child) => {
          if (child instanceof THREE.Mesh) {
            const orig = child.material instanceof THREE.MeshStandardMaterial ? child.material : null;
            const hasEmissiveMap = Boolean(orig && orig.emissiveMap);
            const archMat = new THREE.MeshStandardMaterial({
              color: 0xf7f7f4,
              roughness: 0.34,
              metalness: 0.16,
              normalMap: orig ? orig.normalMap : null,
              normalScale: new THREE.Vector2(1.1, 1.1),
              aoMap: orig ? orig.aoMap : null,
              aoMapIntensity: 0.85,
              emissive: new THREE.Color(0x2248ff),
              emissiveMap: orig ? orig.emissiveMap : null,
              emissiveIntensity: hasEmissiveMap ? 2.1 : 0.03,
            });
            archMat.userData.hasEmissiveMap = hasEmissiveMap;
            child.material = archMat;
            nextMats.push(archMat);

            const edgesGeo = new THREE.EdgesGeometry(child.geometry, 28);
            const edgesMat = new THREE.LineBasicMaterial({
              color: 0x111315,
              transparent: true,
              opacity: 0.22,
            });
            const edgeLines = new THREE.LineSegments(edgesGeo, edgesMat);
            child.add(edgeLines);
            nextEdges.push(edgeLines);
          }
        });

        modelGroup.remove(fallbackCoreGroup);
        icoGeo.dispose();
        icoMat.dispose();
        icoEdges.geometry.dispose();
        icoEdgesMat.dispose();

        wrapper.rotation.y = -0.38;
        modelGroup.add(wrapper);
        materialsRef.current = nextMats;
        edgeLinesRef.current = nextEdges;
        setStatusText('DRAG TO ORBIT // 3D GLTF CORE');
        renderer.render(scene, camera);
      },
      undefined,
      () => {
        if (!disposed) {
          setStatusText('DRAG TO ORBIT 3D CORE');
        }
      },
    );

    function syncSize(): void {
      if (!mount || disposed) return;
      const rect = mount.getBoundingClientRect();
      const w = Math.max(1, rect.width);
      const h = Math.max(1, rect.height);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      renderer.setSize(w, h, false);
    }

    const ro = new ResizeObserver(syncSize);
    ro.observe(mount);
    syncSize();

    const clockStart = performance.now();
    let rafId = 0;

    function animate(now: number): void {
      if (disposed) return;
      const elapsed = (now - clockStart) * 0.001;
      const pageScroll = window.scrollY || window.pageYOffset || 0;

      if (!reducedMotion) {
        modelGroup.rotation.y = elapsed * 0.28 + pageScroll * 0.0022;
        modelGroup.position.y = Math.sin(elapsed * 1.3) * 0.055;

        outerRing.rotation.z = Math.PI * 0.12 + elapsed * 0.18 + pageScroll * 0.001;
        cobaltRing.rotation.z = -elapsed * 0.32 - pageScroll * 0.0016;
      }

      controls.update();
      renderer.render(scene, camera);

      if (!reducedMotion) {
        rafId = requestAnimationFrame(animate);
      }
    }

    const handleControlChange = (): void => {
      if (reducedMotion && !disposed) {
        renderer.render(scene, camera);
      }
    };
    controls.addEventListener('change', handleControlChange);
    rafId = requestAnimationFrame(animate);

    return () => {
      disposed = true;
      cancelAnimationFrame(rafId);
      ro.disconnect();
      controls.removeEventListener('change', handleControlChange);
      controls.dispose();

      scene.traverse((obj) => {
        if (obj instanceof THREE.Mesh || obj instanceof THREE.Line || obj instanceof THREE.LineSegments) {
          obj.geometry.dispose();
          if (Array.isArray(obj.material)) {
            obj.material.forEach((m) => m.dispose());
          } else {
            obj.material.dispose();
          }
        }
      });

      envTarget.dispose();
      roomEnv.dispose();
      pmrem.dispose();
      renderer.dispose();
      if (renderer.domElement.parentElement === mount) {
        mount.removeChild(renderer.domElement);
      }
      renderOnceRef.current = null;
      materialsRef.current = [];
      edgeLinesRef.current = [];
    };
  }, []);

  return (
    <div className="hero-diagram-cell">
      <div className="diagram-header">
        <span>AETHER NEURAL CORE // THREE.JS 3D</span>
        <button
          type="button"
          className={wireframeOnly ? 'wire-toggle-btn active' : 'wire-toggle-btn'}
          aria-pressed={wireframeOnly}
          onClick={() => setWireframeOnly((prev) => !prev)}>
          {wireframeOnly ? 'MESH: PURE WIREFRAME' : 'MESH: SCULPT + CONTOUR'}
        </button>
      </div>

      <div
        className="core-stage"
        role="img"
        aria-label="Interactive Three.js 3D neural visor core representing Aether unified AI memory">
        <div className="hero-3d-mount" ref={mountRef} />
        <div className="model-status-pill">{statusText}</div>
      </div>

      <div className="diagram-footer">
        <div>
          <span className="stat-mini-label">ENGINE</span>
          <span className="stat-mini-val">NATIVE RUST</span>
        </div>
        <div>
          <span className="stat-mini-label">BACKENDS</span>
          <span className="stat-mini-val">VECTOR + GRAPH</span>
        </div>
        <div>
          <span className="stat-mini-label">ISOLATION</span>
          <span className="stat-mini-val">SCOPED TENANT</span>
        </div>
      </div>
    </div>
  );
}
