// peering.bicep
// Establishes bidirectional GLOBAL VNet peering between two VNets that live in
// different Azure regions (but, in this harness, the same resource group).
//
// Global peering keeps cross-region benchmark traffic on the Azure backbone and
// on private IPs -- it never transits the public internet. `allowVirtualNetwork
// Access` is what causes each side's `VirtualNetwork` NSG service tag to include
// the remote VNet's address space, so the network.bicep bench rules "just work"
// across the peering without hard-coded remote prefixes.

@description('Name of the local (primary) VNet. Must already exist in this resource group.')
param primaryVnetName string

@description('Name of the remote (secondary) VNet. Must already exist in this resource group.')
param secondaryVnetName string

resource primaryVnet 'Microsoft.Network/virtualNetworks@2023-11-01' existing = {
  name: primaryVnetName
}

resource secondaryVnet 'Microsoft.Network/virtualNetworks@2023-11-01' existing = {
  name: secondaryVnetName
}

resource peerPrimaryToSecondary 'Microsoft.Network/virtualNetworks/virtualNetworkPeerings@2023-11-01' = {
  parent: primaryVnet
  name: 'peer-to-${secondaryVnetName}'
  properties: {
    remoteVirtualNetwork: {
      id: secondaryVnet.id
    }
    allowVirtualNetworkAccess: true
    allowForwardedTraffic: false
    allowGatewayTransit: false
    useRemoteGateways: false
  }
}

resource peerSecondaryToPrimary 'Microsoft.Network/virtualNetworks/virtualNetworkPeerings@2023-11-01' = {
  parent: secondaryVnet
  name: 'peer-to-${primaryVnetName}'
  properties: {
    remoteVirtualNetwork: {
      id: primaryVnet.id
    }
    allowVirtualNetworkAccess: true
    allowForwardedTraffic: false
    allowGatewayTransit: false
    useRemoteGateways: false
  }
}

@description('Resource ID of the primary->secondary peering.')
output primaryToSecondaryId string = peerPrimaryToSecondary.id

@description('Resource ID of the secondary->primary peering.')
output secondaryToPrimaryId string = peerSecondaryToPrimary.id
